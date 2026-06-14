use anyhow::Result;
use tracing::{debug, info};
use crate::http::Client;

const S3_LIST_URL: &str =
    "https://overturemaps-us-west-2.s3.us-west-2.amazonaws.com/\
     ?list-type=2&prefix=release/&delimiter=/";

/// Fetch the list of available Overture releases from S3.
pub async fn fetch(client: &Client) -> Result<Vec<String>> {
    debug!("fetching available Overture releases");
    let body = client.get_text(S3_LIST_URL).await?;
    let releases = parse_releases(&body)?;
    info!(count = releases.len(), "fetched Overture release list");
    Ok(releases)
}

pub async fn list_and_print(client: &Client) -> Result<()> {
    let releases = fetch(client).await?;
    if releases.is_empty() {
        println!("No releases found.");
    } else {
        println!("Available Overture releases:");
        for r in &releases {
            println!("  {r}");
        }
    }
    Ok(())
}

/// Parse `<Prefix>release/YYYY-MM-DD.N/</Prefix>` entries from S3 XML.
fn parse_releases(xml: &str) -> Result<Vec<String>> {
    let mut releases = Vec::new();
    let mut rest = xml;
    while let Some(pos) = rest.find("<Prefix>release/") {
        rest = &rest[pos + "<Prefix>release/".len()..];
        if let Some(end) = rest.find("</Prefix>") {
            let ver = rest[..end].trim_end_matches('/');
            if !ver.is_empty() {
                releases.push(ver.to_string());
            }
            rest = &rest[end + "</Prefix>".len()..];
        }
    }
    Ok(releases)
}

#[cfg(test)]
mod tests {
    use super::parse_releases;

    #[test]
    fn parses_s3_listing() {
        let xml = r#"
            <Prefix>release/</Prefix>
            <Prefix>release/2026-04-15.0/</Prefix>
            <Prefix>release/2026-05-20.0/</Prefix>
        "#;
        let releases = parse_releases(xml).unwrap();
        assert_eq!(releases, ["2026-04-15.0", "2026-05-20.0"]);
    }
}
