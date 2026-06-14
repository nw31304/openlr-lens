mod adapt;
mod build;
mod cli;
mod extent;
mod extract;
mod http;
mod merge;
mod osm_adapt;
mod osm_extract;
mod parquet_meta;
mod partition;
mod quantize;
mod releases;
mod restrictions;
mod schema;
mod split;
mod tile;

use anyhow::Result;
use clap::Parser;
use cli::{Cli, Command};
use http::RetryConfig;
use tracing::{debug, info};
use tracing_subscriber::{fmt, EnvFilter};

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    let default_level = match cli.verbose {
        0 => "info",
        1 => "debug",
        _ => "trace",
    };
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(default_level));
    fmt().with_env_filter(filter).with_target(false).init();

    debug!("openlrlens-build starting");

    let retry = RetryConfig::new(
        cli.retry_max,
        cli.retry_base_ms,
        cli.retry_max_ms,
        cli.retry_factor,
    );

    match cli.command {
        Command::ListReleases => releases::list_and_print(&http::Client::new(retry)).await?,
        Command::Build(args) => {
            if let Some(n) = args.jobs {
                rayon::ThreadPoolBuilder::new()
                    .num_threads(n)
                    .build_global()
                    .expect("failed to configure rayon thread pool");
                info!(threads = n, "rayon thread pool configured");
            }

            let bbox = extent::resolve(&args.extent)?;

            match args.pbf {
                // ── OSM PBF path ──────────────────────────────────────────────
                Some(pbf_path) => {
                    build::run_osm(
                        &pbf_path,
                        &args.extent,
                        bbox,
                        &args.output,
                        args.tile_zoom,
                    )
                    .await?;
                }

                // ── Overture path ─────────────────────────────────────────────
                None => {
                    let release = args.release.ok_or_else(|| {
                        anyhow::anyhow!("either --pbf or --release must be provided")
                    })?;
                    let client    = http::Client::new(retry);
                    let available = releases::fetch(&client).await?;
                    if !available.contains(&release) {
                        anyhow::bail!(
                            "release '{}' not found. Run `list-releases` to see available releases.",
                            release
                        );
                    }
                    info!(release = %release, extent = %args.extent, "release validated");

                    let schema = schema::load(&args.schema)?;

                    build::run(
                        &release,
                        &args.extent,
                        bbox,
                        &schema,
                        &args.output,
                        &client,
                        args.fetch_concurrency,
                        args.tile_zoom,
                        args.ram_gb,
                        args.bytes_per_segment,
                    )
                    .await?;
                }
            }
        }
    }
    Ok(())
}
