//! The little of the zip format a .docx needs (AWD-53): writing a package
//! with every part deflated, and reading one back with limits on its size,
//! so a crafted file can't make the server inflate gigabytes.

use std::io::{Read, Write};

use flate2::{Compression, read::DeflateDecoder, write::DeflateEncoder};

/// The most parts a package may hold, and the most bytes they may inflate
/// to, each and together.
pub const MAX_ENTRIES: usize = 2_000;
pub const MAX_ENTRY_BYTES: u64 = 16 * 1024 * 1024;
pub const MAX_TOTAL_BYTES: u64 = 48 * 1024 * 1024;

const LOCAL: u32 = 0x0403_4b50;
const CENTRAL: u32 = 0x0201_4b50;
const END: u32 = 0x0605_4b50;

/// A package's parts, deflated, in the order given.
pub fn write(parts: &[(&str, Vec<u8>)]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut central = Vec::new();
    for (name, bytes) in parts {
        let mut encoder = DeflateEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(bytes).expect("deflating to memory");
        let packed = encoder.finish().expect("deflating to memory");
        let crc = crc32fast::hash(bytes);
        let offset = out.len() as u32;
        let header = |signature: u32, out: &mut Vec<u8>| {
            out.extend_from_slice(&signature.to_le_bytes());
            if signature == CENTRAL {
                out.extend_from_slice(&20u16.to_le_bytes()); // made by
            }
            out.extend_from_slice(&20u16.to_le_bytes()); // needed
            out.extend_from_slice(&0x0800u16.to_le_bytes()); // UTF-8 names
            out.extend_from_slice(&8u16.to_le_bytes()); // deflate
            out.extend_from_slice(&0u16.to_le_bytes()); // time
            out.extend_from_slice(&0x21u16.to_le_bytes()); // 1980-01-01
            out.extend_from_slice(&crc.to_le_bytes());
            out.extend_from_slice(&(packed.len() as u32).to_le_bytes());
            out.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
            out.extend_from_slice(&(name.len() as u16).to_le_bytes());
            out.extend_from_slice(&0u16.to_le_bytes()); // extra
            if signature == CENTRAL {
                out.extend_from_slice(&0u16.to_le_bytes()); // comment
                out.extend_from_slice(&0u16.to_le_bytes()); // disk
                out.extend_from_slice(&0u16.to_le_bytes()); // internal
                out.extend_from_slice(&0u32.to_le_bytes()); // external
                out.extend_from_slice(&offset.to_le_bytes());
            }
            out.extend_from_slice(name.as_bytes());
        };
        header(LOCAL, &mut out);
        out.extend_from_slice(&packed);
        header(CENTRAL, &mut central);
    }
    let start = out.len() as u32;
    out.extend_from_slice(&central);
    out.extend_from_slice(&END.to_le_bytes());
    out.extend_from_slice(&[0; 4]); // disks
    out.extend_from_slice(&(parts.len() as u16).to_le_bytes());
    out.extend_from_slice(&(parts.len() as u16).to_le_bytes());
    out.extend_from_slice(&(central.len() as u32).to_le_bytes());
    out.extend_from_slice(&start.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out
}

/// Why a package can't be read.
#[derive(Debug, PartialEq, Eq)]
pub enum ZipError {
    NotZip,
    TooBig,
    Unsupported,
}

struct Entry {
    name: String,
    method: u16,
    packed: u64,
    size: u64,
    offset: u64,
}

fn u16_at(bytes: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(bytes.get(at..at + 2)?.try_into().ok()?))
}

fn u32_at(bytes: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(bytes.get(at..at + 4)?.try_into().ok()?))
}

/// A package: the names of its parts, and each one's bytes on request.
pub struct Package<'a> {
    bytes: &'a [u8],
    entries: Vec<Entry>,
}

impl<'a> Package<'a> {
    pub fn open(bytes: &'a [u8]) -> Result<Self, ZipError> {
        // The end record is in the last 64 KiB (it may carry a comment).
        let floor = bytes.len().saturating_sub(22 + 65_535);
        let end = (floor..bytes.len().saturating_sub(21))
            .rev()
            .find(|&at| u32_at(bytes, at) == Some(END))
            .ok_or(ZipError::NotZip)?;
        let count = usize::from(u16_at(bytes, end + 10).ok_or(ZipError::NotZip)?);
        let mut at = u32_at(bytes, end + 16).ok_or(ZipError::NotZip)? as usize;
        if count > MAX_ENTRIES {
            return Err(ZipError::TooBig);
        }
        let mut entries = Vec::with_capacity(count);
        let mut total = 0u64;
        for _ in 0..count {
            if u32_at(bytes, at) != Some(CENTRAL) {
                return Err(ZipError::NotZip);
            }
            let flags = u16_at(bytes, at + 8).ok_or(ZipError::NotZip)?;
            let method = u16_at(bytes, at + 10).ok_or(ZipError::NotZip)?;
            let packed = u64::from(u32_at(bytes, at + 20).ok_or(ZipError::NotZip)?);
            let size = u64::from(u32_at(bytes, at + 24).ok_or(ZipError::NotZip)?);
            let name_length = usize::from(u16_at(bytes, at + 28).ok_or(ZipError::NotZip)?);
            let extra = usize::from(u16_at(bytes, at + 30).ok_or(ZipError::NotZip)?);
            let comment = usize::from(u16_at(bytes, at + 32).ok_or(ZipError::NotZip)?);
            let offset = u64::from(u32_at(bytes, at + 42).ok_or(ZipError::NotZip)?);
            let name = bytes
                .get(at + 46..at + 46 + name_length)
                .ok_or(ZipError::NotZip)?;
            // Encrypted parts and zip64 sizes are beyond what a page needs.
            if flags & 1 != 0 || packed == u64::from(u32::MAX) || size == u64::from(u32::MAX) {
                return Err(ZipError::Unsupported);
            }
            if size > MAX_ENTRY_BYTES {
                return Err(ZipError::TooBig);
            }
            total += size;
            if total > MAX_TOTAL_BYTES {
                return Err(ZipError::TooBig);
            }
            entries.push(Entry {
                name: String::from_utf8_lossy(name).into_owned(),
                method,
                packed,
                size,
                offset,
            });
            at += 46 + name_length + extra + comment;
        }
        Ok(Self { bytes, entries })
    }

    pub fn has(&self, name: &str) -> bool {
        self.entries.iter().any(|entry| entry.name == name)
    }

    /// A part's bytes, or None when the package hasn't got it.
    pub fn read(&self, name: &str) -> Result<Option<Vec<u8>>, ZipError> {
        let Some(entry) = self.entries.iter().find(|entry| entry.name == name) else {
            return Ok(None);
        };
        let at = entry.offset as usize;
        if u32_at(self.bytes, at) != Some(LOCAL) {
            return Err(ZipError::NotZip);
        }
        let name_length = usize::from(u16_at(self.bytes, at + 26).ok_or(ZipError::NotZip)?);
        let extra = usize::from(u16_at(self.bytes, at + 28).ok_or(ZipError::NotZip)?);
        let start = at + 30 + name_length + extra;
        let data = self
            .bytes
            .get(start..start + entry.packed as usize)
            .ok_or(ZipError::NotZip)?;
        // Read one byte past the declared size, to catch a part that lies.
        let mut out = Vec::with_capacity(entry.size.min(MAX_ENTRY_BYTES) as usize);
        match entry.method {
            0 => out.extend_from_slice(data),
            8 => {
                DeflateDecoder::new(data)
                    .take(entry.size + 1)
                    .read_to_end(&mut out)
                    .map_err(|_| ZipError::NotZip)?;
            }
            _ => return Err(ZipError::Unsupported),
        }
        if out.len() as u64 != entry.size {
            return Err(ZipError::NotZip);
        }
        Ok(Some(out))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_written_package_reads_back() {
        let bytes = write(&[
            ("a.xml", b"<a/>".to_vec()),
            ("dir/b.txt", vec![b'x'; 10_000]),
        ]);
        let package = Package::open(&bytes).expect("opens");
        assert!(package.has("dir/b.txt"));
        assert_eq!(package.read("a.xml").expect("test").expect("test"), b"<a/>");
        assert_eq!(
            package
                .read("dir/b.txt")
                .expect("test")
                .expect("test")
                .len(),
            10_000
        );
        assert_eq!(package.read("missing").expect("test"), None);
    }

    #[test]
    fn junk_and_lies_are_refused() {
        assert_eq!(Package::open(b"not a zip").err(), Some(ZipError::NotZip));
        // A part that says it's small but inflates further.
        let mut bytes = write(&[("a", vec![b'x'; 5_000])]);
        let central = bytes
            .windows(4)
            .position(|window| window == CENTRAL.to_le_bytes())
            .expect("test");
        bytes[central + 24..central + 28].copy_from_slice(&10u32.to_le_bytes());
        let package = Package::open(&bytes).expect("opens");
        assert_eq!(package.read("a").err(), Some(ZipError::NotZip));
    }
}
