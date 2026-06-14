//! Minimal parquet footer reader for bbox-based file pre-filtering.
//!
//! Uses two HTTP Range requests per file: first fetches the last 8 bytes to read
//! `footer_length` from the parquet trailer, then fetches exactly that many bytes
//! to get the complete Thrift-encoded FileMetaData.  Parses row-group bbox statistics
//! and reports whether any row group might overlap the query bbox.  On any parse
//! failure the function conservatively returns `true`.
//!
//! **Encoding**: Parquet uses Thrift Compact Protocol for footer serialization,
//! not Binary Protocol.  Key differences from Binary:
//!  - Field headers are delta-encoded (1 byte for small deltas)
//!  - Integers are zigzag+varint encoded
//!  - Strings/binary use varint length prefix
//!  - BOOL value embedded in the type nibble (no extra byte)
//!  - DOUBLE is 8-byte little-endian (not big-endian)
//!  - List count packed with element type in one byte for small lists

use tracing::debug;

use crate::{extent::Bbox, http::Client};

// ── Public API ────────────────────────────────────────────────────────────────

/// Returns `true` if the parquet file at `url` might contain segments overlapping `bbox`.
///
/// Issues two HTTP Range requests (trailer + full footer), parses row-group bbox
/// statistics, and prunes files with no overlapping row groups.  Conservative: returns
/// `true` on any error or missing statistics.
pub async fn file_may_overlap(url: &str, bbox: Bbox, client: &Client) -> bool {
    match check_inner(url, bbox, client).await {
        Ok(v) => v,
        Err(e) => {
            debug!(url, error = %e, "parquet bbox pre-check failed, assuming overlap");
            true
        }
    }
}

async fn check_inner(
    url: &str,
    bbox: Bbox,
    client: &Client,
) -> Result<bool, anyhow::Error> {
    // Step 1: fetch the 8-byte parquet trailer to learn footer_length.
    let trailer = client.get_range_bytes_suffix(url, 8).await?;
    if trailer.len() < 8 { anyhow::bail!("file too short"); }
    if &trailer[4..] != b"PAR1" { anyhow::bail!("missing PAR1 magic"); }
    let footer_len = u32::from_le_bytes(trailer[..4].try_into().unwrap()) as usize;
    if footer_len == 0 { anyhow::bail!("zero-length footer"); }

    // Step 2: fetch the full footer (footer_len bytes) + 8-byte trailer.
    let fetch_size = (footer_len + 8) as u64;
    let tail = client.get_range_bytes_suffix(url, fetch_size).await?;
    let n = tail.len();
    if n < 8 { anyhow::bail!("footer fetch too short"); }
    let footer_bytes = &tail[..n - 8]; // strip trailing 4-byte length + PAR1

    Ok(parse_overlaps(footer_bytes, bbox).unwrap_or(true))
}

// ── Thrift Compact Protocol type nibbles ──────────────────────────────────────

const CP_BOOL_TRUE:  u8 = 1;
const CP_BOOL_FALSE: u8 = 2;
const CP_BYTE:       u8 = 3;
const CP_I16:        u8 = 4;
const CP_I32:        u8 = 5;
const CP_I64:        u8 = 6;
const CP_DOUBLE:     u8 = 7;
const CP_BINARY:     u8 = 8;
const CP_LIST:       u8 = 9;
const CP_SET:        u8 = 10;
const CP_MAP:        u8 = 11;
const CP_STRUCT:     u8 = 12;

// Parquet physical Type enum values (encoded as zigzag-varint i32).
const PARQUET_FLOAT:  i32 = 4;
const PARQUET_DOUBLE: i32 = 5;

// ── Cursor ────────────────────────────────────────────────────────────────────

struct Cur<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Cur<'a> {
    fn new(buf: &'a [u8]) -> Self { Self { buf, pos: 0 } }

    fn read_byte(&mut self) -> Option<u8> {
        let b = *self.buf.get(self.pos)?;
        self.pos += 1;
        Some(b)
    }

    /// Read an unsigned varint (up to 10 bytes for 64-bit values).
    fn read_varint_u64(&mut self) -> Option<u64> {
        let mut result = 0u64;
        let mut shift = 0u32;
        loop {
            let b = self.read_byte()?;
            result |= ((b & 0x7F) as u64) << shift;
            if b & 0x80 == 0 { return Some(result); }
            shift = shift.checked_add(7)?;
            if shift >= 64 { return None; }
        }
    }

    /// Read a zigzag-encoded varint as i32.
    fn read_zigzag_i32(&mut self) -> Option<i32> {
        let n = self.read_varint_u64()? as u32;
        Some(((n >> 1) as i32) ^ -((n & 1) as i32))
    }

    /// Read a Compact Protocol field header.
    ///
    /// `last_fid` carries the previous field ID in the current struct (starts at 0).
    /// Returns `(type_nibble, field_id)`.  `type_nibble == 0` is STOP; field_id is 0.
    fn field_header(&mut self, last_fid: &mut i16) -> Option<(u8, i16)> {
        let b = self.read_byte()?;
        if b == 0 { return Some((0, 0)); } // STOP

        let typ   = b & 0x0F;
        let delta = (b >> 4) & 0x0F;
        let fid = if delta == 0 {
            // Long form: 2-byte little-endian field ID follows.
            let lo = self.read_byte()? as i16;
            let hi = self.read_byte()? as i16;
            (hi << 8) | lo
        } else {
            *last_fid + delta as i16
        };
        *last_fid = fid;
        Some((typ, fid))
    }

    /// Read a Compact Protocol list header. Returns `(element_type_nibble, count)`.
    fn list_header(&mut self) -> Option<(u8, usize)> {
        let b = self.read_byte()?;
        let elem_type = b & 0x0F;
        let count = if (b >> 4) < 0x0F {
            (b >> 4) as usize
        } else {
            self.read_varint_u64()? as usize
        };
        Some((elem_type, count))
    }

    /// Read a Compact Protocol BINARY/STRING value (varint length + bytes).
    fn binary(&mut self) -> Option<&'a [u8]> {
        let len = self.read_varint_u64()? as usize;
        let end = self.pos.checked_add(len)?;
        if end > self.buf.len() { return None; }
        let s = &self.buf[self.pos..end];
        self.pos = end;
        Some(s)
    }

    /// Skip a Compact Protocol field value of the given type nibble.
    fn skip(&mut self, typ: u8) -> Option<()> {
        match typ {
            CP_BOOL_TRUE | CP_BOOL_FALSE => {}             // value in type nibble, no bytes
            CP_BYTE               => { self.read_byte()?; }
            CP_I16 | CP_I32 | CP_I64 => { self.read_varint_u64()?; }
            CP_DOUBLE             => { self.pos = self.pos.checked_add(8)?; } // 8-byte LE
            CP_BINARY             => {
                let n = self.read_varint_u64()? as usize;
                self.pos = self.pos.checked_add(n)?;
            }
            CP_LIST | CP_SET => {
                let (et, n) = self.list_header()?;
                for _ in 0..n { self.skip(et)?; }
            }
            CP_MAP => {
                let n = self.read_varint_u64()? as usize;
                if n > 0 {
                    let b = self.read_byte()?;
                    let kt = (b >> 4) & 0x0F;
                    let vt = b & 0x0F;
                    for _ in 0..n { self.skip(kt)?; self.skip(vt)?; }
                }
            }
            CP_STRUCT => {
                let mut fid = 0i16;
                loop {
                    let (t, _) = self.field_header(&mut fid)?;
                    if t == 0 { break; }
                    self.skip(t)?;
                }
            }
            13 => { self.pos = self.pos.checked_add(16)?; } // UUID: 16 bytes
            _ => return None,
        }
        if self.pos > self.buf.len() { return None; }
        Some(())
    }
}

// ── Row-group bounding box ────────────────────────────────────────────────────

#[derive(Default)]
struct RgBbox {
    west:  Option<f64>, // min(xmin) across RG = xmin column's min statistic
    east:  Option<f64>, // max(xmax) across RG = xmax column's max statistic
    south: Option<f64>, // min(ymin) across RG = ymin column's min statistic
    north: Option<f64>, // max(ymax) across RG = ymax column's max statistic
}

impl RgBbox {
    fn overlaps(&self, q: Bbox) -> bool {
        match (self.west, self.east, self.south, self.north) {
            (Some(w), Some(e), Some(s), Some(n)) => {
                e >= q.west && w <= q.east && n >= q.south && s <= q.north
            }
            _ => true, // missing stats → conservative
        }
    }
}

// ── Parse logic ───────────────────────────────────────────────────────────────

/// Returns `Some(true)` if any row group might overlap `bbox`, `Some(false)` if none
/// can, or `None` on a parse error (caller should treat as `true`).
fn parse_overlaps(thrift: &[u8], bbox: Bbox) -> Option<bool> {
    let mut cur = Cur::new(thrift);
    let mut fid = 0i16;
    // FileMetaData: field 4 = list<RowGroup>
    // (field 1=version i32, field 2=schema list<SchemaElement>, field 3=num_rows i64)
    loop {
        let (t, f) = cur.field_header(&mut fid)?;
        if t == 0 { return Some(false); }
        if t == CP_LIST && f == 4 {
            let (et, n) = cur.list_header()?;
            for _ in 0..n {
                if et != CP_STRUCT { cur.skip(et)?; continue; }
                if parse_row_group(&mut cur, bbox)? { return Some(true); }
            }
            return Some(false);
        }
        cur.skip(t)?;
    }
}

fn parse_row_group(cur: &mut Cur, bbox: Bbox) -> Option<bool> {
    let mut rg = RgBbox::default();
    let mut fid = 0i16;
    // RowGroup: field 1 = list<ColumnChunk>
    loop {
        let (t, f) = cur.field_header(&mut fid)?;
        if t == 0 { break; }
        if t == CP_LIST && f == 1 {
            let (et, n) = cur.list_header()?;
            for _ in 0..n {
                if et != CP_STRUCT { cur.skip(et)?; continue; }
                parse_column_chunk(cur, &mut rg)?;
            }
        } else {
            cur.skip(t)?;
        }
    }
    Some(rg.overlaps(bbox))
}

fn parse_column_chunk(cur: &mut Cur, rg: &mut RgBbox) -> Option<()> {
    let mut fid = 0i16;
    // ColumnChunk: field 3 = ColumnMetaData (struct)
    loop {
        let (t, f) = cur.field_header(&mut fid)?;
        if t == 0 { break; }
        if t == CP_STRUCT && f == 3 {
            parse_column_metadata(cur, rg)?;
        } else {
            cur.skip(t)?;
        }
    }
    Some(())
}

fn parse_column_metadata(cur: &mut Cur, rg: &mut RgBbox) -> Option<()> {
    let mut col_type: i32 = -1;
    let mut path:     Vec<String> = Vec::new();
    let mut stat_min: Option<Vec<u8>> = None;
    let mut stat_max: Option<Vec<u8>> = None;
    let mut fid = 0i16;

    loop {
        let (t, f) = cur.field_header(&mut fid)?;
        if t == 0 { break; }
        match (t, f) {
            (CP_I32, 1) => {
                col_type = cur.read_zigzag_i32()?;
            }
            (CP_LIST, 3) => {
                let (et, n) = cur.list_header()?;
                path.clear();
                for _ in 0..n {
                    if et == CP_BINARY {
                        let b = cur.binary()?;
                        path.push(String::from_utf8_lossy(b).into_owned());
                    } else {
                        cur.skip(et)?;
                    }
                }
            }
            (CP_STRUCT, 12) => {
                parse_statistics(cur, &mut stat_min, &mut stat_max)?;
            }
            _ => { cur.skip(t)?; }
        }
    }

    update_rg_bbox(rg, &path, col_type, stat_min.as_deref(), stat_max.as_deref());
    Some(())
}

fn parse_statistics(
    cur: &mut Cur,
    min_out: &mut Option<Vec<u8>>,
    max_out: &mut Option<Vec<u8>>,
) -> Option<()> {
    let mut old_max: Option<Vec<u8>> = None;
    let mut old_min: Option<Vec<u8>> = None;
    let mut new_max: Option<Vec<u8>> = None;
    let mut new_min: Option<Vec<u8>> = None;
    let mut fid = 0i16;

    loop {
        let (t, f) = cur.field_header(&mut fid)?;
        if t == 0 { break; }
        match (t, f) {
            (CP_BINARY, 1) => { old_max = Some(cur.binary()?.to_vec()); }
            (CP_BINARY, 2) => { old_min = Some(cur.binary()?.to_vec()); }
            (CP_BINARY, 5) => { new_max = Some(cur.binary()?.to_vec()); }
            (CP_BINARY, 6) => { new_min = Some(cur.binary()?.to_vec()); }
            _ => { cur.skip(t)?; }
        }
    }

    // Prefer new-style (fields 5+6) over old-style (fields 1+2)
    *min_out = new_min.or(old_min);
    *max_out = new_max.or(old_max);
    Some(())
}

fn update_rg_bbox(
    rg: &mut RgBbox,
    path: &[String],
    col_type: i32,
    stat_min: Option<&[u8]>,
    stat_max: Option<&[u8]>,
) {
    // Overture bbox path: ["bbox", "xmin"] / ["bbox", "xmax"] / ["bbox", "ymin"] / ["bbox", "ymax"]
    if path.len() < 2 || path[0] != "bbox" { return; }
    let leaf = path[1].as_str();

    let as_f64 = |bytes: &[u8]| -> Option<f64> {
        match (col_type, bytes.len()) {
            (PARQUET_FLOAT,  4) => Some(f32::from_le_bytes(bytes.try_into().ok()?) as f64),
            (PARQUET_DOUBLE, 8) => Some(f64::from_le_bytes(bytes.try_into().ok()?)),
            _ => None,
        }
    };

    match leaf {
        "xmin" => rg.west  = stat_min.and_then(as_f64),
        "xmax" => rg.east  = stat_max.and_then(as_f64),
        "ymin" => rg.south = stat_min.and_then(as_f64),
        "ymax" => rg.north = stat_max.and_then(as_f64),
        _ => {}
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── Thrift Compact Protocol encoding helpers ──────────────────────────────

    fn varint(b: &mut Vec<u8>, v: u64) {
        let mut v = v;
        loop {
            let byte = (v & 0x7F) as u8;
            v >>= 7;
            if v == 0 { b.push(byte); return; }
            b.push(byte | 0x80);
        }
    }

    fn zigzag_i32(v: i32) -> u64 {
        ((v << 1) ^ (v >> 31)) as u32 as u64
    }

    fn stop(b: &mut Vec<u8>) { b.push(0x00); }

    fn field_i32(b: &mut Vec<u8>, last_fid: &mut i16, fid: i16, v: i32) {
        write_field_header(b, last_fid, fid, CP_I32);
        varint(b, zigzag_i32(v));
    }

    fn field_binary(b: &mut Vec<u8>, last_fid: &mut i16, fid: i16, data: &[u8]) {
        write_field_header(b, last_fid, fid, CP_BINARY);
        varint(b, data.len() as u64);
        b.extend_from_slice(data);
    }

    fn field_list_string(b: &mut Vec<u8>, last_fid: &mut i16, fid: i16, strings: &[&str]) {
        write_field_header(b, last_fid, fid, CP_LIST);
        // List header: count < 15 → (count << 4) | elem_type
        assert!(strings.len() < 15);
        b.push(((strings.len() as u8) << 4) | CP_BINARY);
        for s in strings {
            varint(b, s.len() as u64);
            b.extend_from_slice(s.as_bytes());
        }
    }

    fn field_struct_header(b: &mut Vec<u8>, last_fid: &mut i16, fid: i16) {
        write_field_header(b, last_fid, fid, CP_STRUCT);
    }

    fn field_list_struct_header(b: &mut Vec<u8>, last_fid: &mut i16, fid: i16, count: usize) {
        write_field_header(b, last_fid, fid, CP_LIST);
        if count < 15 {
            b.push(((count as u8) << 4) | CP_STRUCT);
        } else {
            b.push(0xF0 | CP_STRUCT);
            varint(b, count as u64);
        }
    }

    fn write_field_header(b: &mut Vec<u8>, last_fid: &mut i16, fid: i16, typ: u8) {
        let delta = fid - *last_fid;
        if delta > 0 && delta <= 15 {
            b.push(((delta as u8) << 4) | typ);
        } else {
            b.push(typ); // long form: delta=0
            b.extend_from_slice(&fid.to_le_bytes());
        }
        *last_fid = fid;
    }

    fn append_bbox_column_chunk(b: &mut Vec<u8>, leaf: &str, stat_min: f32, stat_max: f32) {
        // ColumnChunk struct element — no field header (it's a list element)
        // Field 3: ColumnMetaData (STRUCT)
        let mut fid = 0i16;
        field_struct_header(b, &mut fid, 3);
        {
            // ColumnMetaData fields
            let mut md_fid = 0i16;
            field_i32(b, &mut md_fid, 1, PARQUET_FLOAT);          // type = FLOAT
            field_list_string(b, &mut md_fid, 3, &["bbox", leaf]); // path_in_schema
            field_struct_header(b, &mut md_fid, 12);               // Statistics
            {
                // Statistics fields
                let mut st_fid = 0i16;
                field_binary(b, &mut st_fid, 5, &stat_max.to_le_bytes()); // max_value
                field_binary(b, &mut st_fid, 6, &stat_min.to_le_bytes()); // min_value
                stop(b); // end Statistics
            }
            stop(b); // end ColumnMetaData
        }
        stop(b); // end ColumnChunk
    }

    fn file_meta_one_rg(xmin: f32, xmax: f32, ymin: f32, ymax: f32) -> Vec<u8> {
        let mut b = Vec::new();
        let mut fid = 0i16;

        field_i32(&mut b, &mut fid, 1, 2);                       // version = 2
        field_list_struct_header(&mut b, &mut fid, 4, 1);        // field 4: 1 row group
        {
            // RowGroup struct element
            let mut rg_fid = 0i16;
            field_list_struct_header(&mut b, &mut rg_fid, 1, 4); // field 1: 4 col chunks
            append_bbox_column_chunk(&mut b, "xmin", xmin, xmax);
            append_bbox_column_chunk(&mut b, "xmax", xmin, xmax);
            append_bbox_column_chunk(&mut b, "ymin", ymin, ymax);
            append_bbox_column_chunk(&mut b, "ymax", ymin, ymax);
            stop(&mut b); // end RowGroup
        }
        stop(&mut b); // end FileMetaData
        b
    }

    // ── Test cases ────────────────────────────────────────────────────────────

    #[test]
    fn empty_file_meta_no_overlap() {
        let mut b = Vec::new();
        let mut fid = 0i16;
        field_i32(&mut b, &mut fid, 1, 2);              // version
        field_list_struct_header(&mut b, &mut fid, 4, 0); // 0 row groups
        stop(&mut b);
        assert_eq!(
            parse_overlaps(&b, Bbox { west: 0.0, south: 0.0, east: 1.0, north: 1.0 }),
            Some(false)
        );
    }

    #[test]
    fn rg_inside_bbox_overlaps() {
        let nz = Bbox { west: 166.0, south: -47.5, east: 178.5, north: -34.0 };
        let fm = file_meta_one_rg(166.0, 178.5, -47.5, -34.0);
        assert_eq!(parse_overlaps(&fm, nz), Some(true));
    }

    #[test]
    fn rg_in_europe_does_not_overlap_nz() {
        let nz = Bbox { west: 166.0, south: -47.5, east: 178.5, north: -34.0 };
        let fm = file_meta_one_rg(-10.0, 40.0, 35.0, 70.0);
        assert_eq!(parse_overlaps(&fm, nz), Some(false));
    }

    #[test]
    fn rg_partially_overlaps() {
        let bbox = Bbox { west: 170.0, south: -45.0, east: 180.0, north: -35.0 };
        let fm = file_meta_one_rg(175.0, 200.0, -50.0, -40.0);
        assert_eq!(parse_overlaps(&fm, bbox), Some(true));
    }

    #[test]
    fn corrupt_bytes_returns_none() {
        assert_eq!(
            parse_overlaps(b"not thrift", Bbox { west: 0.0, south: 0.0, east: 1.0, north: 1.0 }),
            None
        );
    }
}
