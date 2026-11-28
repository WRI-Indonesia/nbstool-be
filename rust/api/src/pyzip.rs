//! The .docx container as python-docx saves it: `zipfile.ZipFile(f, 'w', ZIP_DEFLATED)` and one
//! `writestr(name, blob)` per part (CPython 3.12 `zipfile`, zlib level 6 raw deflate through the
//! real zlib via flate2's `zlib` backend). nbstool-docx proves the part bytes; this makes the
//! archive bytes equal too (headers, deflate streams, central directory), up to the timestamps.

use std::io::{Cursor, Read};

/// `ZipInfo.create_system`: 0 on Windows, 3 elsewhere (staging is Linux).
const CREATE_SYSTEM: u8 = if cfg!(windows) { 0 } else { 3 };

/// `compressobj(6, DEFLATED, -15)`: `compress(data)` then `flush()`.
fn deflate(data: &[u8]) -> Result<Vec<u8>, String> {
    use flate2::{Compress, Compression, FlushCompress, Status};
    let mut c = Compress::new(Compression::new(6), false);
    let mut out = Vec::with_capacity(data.len() / 2 + 64);
    let mut consumed = 0usize;
    // Z_NO_FLUSH over all input, as zipfile's single compress() call
    while consumed < data.len() {
        out.reserve(64 * 1024);
        let before = c.total_in();
        c.compress_vec(&data[consumed..], &mut out, FlushCompress::None)
            .map_err(|e| e.to_string())?;
        consumed += (c.total_in() - before) as usize;
    }
    loop {
        out.reserve(64 * 1024);
        match c
            .compress_vec(&[], &mut out, FlushCompress::Finish)
            .map_err(|e| e.to_string())?
        {
            Status::StreamEnd => return Ok(out),
            Status::Ok | Status::BufError => {}
        }
    }
}

/// `time.localtime()` as zipfile's DOS date and time.
fn dos_now() -> (u16, u16) {
    use chrono::{Datelike, Timelike};
    let now = chrono::Local::now();
    let date = (((now.year() - 1980) as u16) << 9) | ((now.month() as u16) << 5) | now.day() as u16;
    let time =
        ((now.hour() as u16) << 11) | ((now.minute() as u16) << 5) | (now.second() as u16 / 2);
    (date, time)
}

/// Re-writes `docx` entry by entry, in its order, as zipfile writes them.
pub fn python_docx_zip(docx: &[u8]) -> Result<Vec<u8>, String> {
    let (date, time) = dos_now();
    zip_at(docx, date, time)
}

fn zip_at(docx: &[u8], date: u16, time: u16) -> Result<Vec<u8>, String> {
    let mut archive = zip::ZipArchive::new(Cursor::new(docx)).map_err(|e| e.to_string())?;
    let mut out = Vec::with_capacity(docx.len() + 1024);
    let mut central = Vec::new();
    let mut count: u16 = 0;
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).map_err(|e| e.to_string())?;
        let name = entry.name().to_string();
        let mut data = Vec::with_capacity(entry.size() as usize);
        entry.read_to_end(&mut data).map_err(|e| e.to_string())?;
        let packed = deflate(&data)?;
        let crc = crc32fast::hash(&data);
        let flags: u16 = if name.is_ascii() { 0 } else { 0x800 };
        let offset = out.len() as u32;
        // local file header
        out.extend_from_slice(b"PK\x03\x04");
        for v in [20u16, flags, 8, time, date] {
            out.extend_from_slice(&v.to_le_bytes());
        }
        for v in [crc, packed.len() as u32, data.len() as u32] {
            out.extend_from_slice(&v.to_le_bytes());
        }
        out.extend_from_slice(&(name.len() as u16).to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(name.as_bytes());
        out.extend_from_slice(&packed);
        // central directory record
        central.extend_from_slice(b"PK\x01\x02");
        central.extend_from_slice(&[20, CREATE_SYSTEM, 20, 0]);
        for v in [flags, 8, time, date] {
            central.extend_from_slice(&v.to_le_bytes());
        }
        for v in [crc, packed.len() as u32, data.len() as u32] {
            central.extend_from_slice(&v.to_le_bytes());
        }
        for v in [name.len() as u16, 0, 0, 0, 0] {
            central.extend_from_slice(&v.to_le_bytes());
        }
        // writestr: external_attr = 0o600 << 16
        central.extend_from_slice(&(0o600u32 << 16).to_le_bytes());
        central.extend_from_slice(&offset.to_le_bytes());
        central.extend_from_slice(name.as_bytes());
        count += 1;
    }
    let cd_offset = out.len() as u32;
    let cd_size = central.len() as u32;
    out.extend_from_slice(&central);
    out.extend_from_slice(b"PK\x05\x06");
    for v in [0u16, 0, count, count] {
        out.extend_from_slice(&v.to_le_bytes());
    }
    out.extend_from_slice(&cd_size.to_le_bytes());
    out.extend_from_slice(&cd_offset.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    Ok(out)
}

#[cfg(test)]
mod tests {
    /// The CCB docx the original served in a recording, re-zipped from its own entries with its
    /// own timestamp: byte-equal.
    #[test]
    fn rezips_python_docx_byte_for_byte() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../app-cases/http/dl-docx-01-ccb.body"
        );
        let Ok(original) = std::fs::read(path) else {
            return;
        };
        let date = u16::from_le_bytes([original[12], original[13]]);
        let time = u16::from_le_bytes([original[10], original[11]]);
        let again = super::zip_at(&original, date, time).unwrap();
        assert_eq!(again.len(), original.len());
        assert!(again == original);
    }

    #[test]
    fn deflate_is_zlib_raw_level6() {
        // zlib.compressobj(6, 8, -15); c.compress(b'hello hello hello') + c.flush()
        assert_eq!(
            super::deflate(b"hello hello hello").unwrap(),
            vec![0xcb, 0x48, 0xcd, 0xc9, 0xc9, 0x57, 0xc8, 0x40, 0x90, 0x00]
        );
    }
}
