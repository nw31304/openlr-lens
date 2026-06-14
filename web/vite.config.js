import { defineConfig } from 'vite';
import path from 'path';
import fs from 'fs';

export default defineConfig({
  // Serve the pipeline output directory at /tiles/ with proper Range-request support.
  // PMTiles requires Range requests; Vite's built-in static server doesn't handle them
  // for files outside the project root, so we wire up a small middleware.
  plugins: [{
    name: 'serve-tiles',
    configureServer(server) {
      const tilesDir = path.resolve(__dirname, '../out');
      server.middlewares.use('/tiles', (req, res, next) => {
        const filePath = path.join(tilesDir, decodeURIComponent(req.url.slice(1)));
        if (!fs.existsSync(filePath) || !filePath.startsWith(tilesDir)) {
          return next();
        }
        const stat = fs.statSync(filePath);
        const rangeHeader = req.headers['range'];
        if (rangeHeader) {
          const [, startStr, endStr] = rangeHeader.match(/bytes=(\d+)-(\d*)/) ?? [];
          const start = parseInt(startStr, 10);
          const end   = endStr ? parseInt(endStr, 10) : stat.size - 1;
          res.writeHead(206, {
            'Content-Range':  `bytes ${start}-${end}/${stat.size}`,
            'Accept-Ranges':  'bytes',
            'Content-Length': end - start + 1,
            'Content-Type':   'application/octet-stream',
            'Access-Control-Allow-Origin': '*',
          });
          fs.createReadStream(filePath, { start, end }).pipe(res);
        } else {
          res.writeHead(200, {
            'Content-Length': stat.size,
            'Content-Type':   'application/octet-stream',
            'Access-Control-Allow-Origin': '*',
          });
          fs.createReadStream(filePath).pipe(res);
        }
      });
    },
  }],
});
