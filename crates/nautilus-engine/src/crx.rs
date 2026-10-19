//! The CRX3 container the store serves extensions in: a header with the
//! publisher's key and the extension id, then a zip of the extension.
//! Only what is needed to unpack one is parsed; the signatures are not
//! verified (the download comes over HTTPS from the store itself, and the
//! unpacked extension is then loaded as a local one).

use std::fmt;

use sha2::{Digest, Sha256};

/// What a CRX file holds, once its header is read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Crx<'a> {
    /// The extension id Chromium signed into the header.
    pub id: String,
    /// The publisher's public key (DER): the one the id is the hash of,
    /// which an unpacked copy needs as its `key` to keep that id. A store
    /// CRX also carries the store's own signing key, which is not it.
    pub public_key: Vec<u8>,
    /// The zip archive of the extension's files.
    pub archive: &'a [u8],
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum CrxError {
    NotCrx,
    Version(u32),
    Truncated,
    Header(&'static str),
}

impl fmt::Display for CrxError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CrxError::NotCrx => f.write_str("not a CRX file"),
            CrxError::Version(v) => write!(f, "CRX version {v} is not supported (only 3)"),
            CrxError::Truncated => f.write_str("the CRX file is truncated"),
            CrxError::Header(what) => write!(f, "the CRX header is malformed: {what}"),
        }
    }
}

const MAGIC: &[u8; 4] = b"Cr24";

/// Parse a CRX3 file.
pub(crate) fn parse(bytes: &[u8]) -> Result<Crx<'_>, CrxError> {
    if bytes.len() < 12 {
        return Err(if bytes.starts_with(MAGIC) {
            CrxError::Truncated
        } else {
            CrxError::NotCrx
        });
    }
    if &bytes[..4] != MAGIC {
        return Err(CrxError::NotCrx);
    }
    let version = u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]);
    if version != 3 {
        return Err(CrxError::Version(version));
    }
    let header_len = u32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]) as usize;
    let rest = &bytes[12..];
    if rest.len() < header_len {
        return Err(CrxError::Truncated);
    }
    let (header, archive) = rest.split_at(header_len);
    let (id, public_key) = parse_header(header)?;
    Ok(Crx {
        id,
        public_key,
        archive,
    })
}

/// The `CrxFileHeader` protobuf: fields 2 and 3 (repeated
/// `AsymmetricKeyProof`, RSA and ECDSA, whose field 1 is the public key)
/// and field 10000 (`SignedData`, whose field 1 is the 16-byte id). The
/// publisher's key is the proof whose key hashes to that id.
fn parse_header(header: &[u8]) -> Result<(String, Vec<u8>), CrxError> {
    let mut keys = Vec::new();
    let mut crx_id = None;
    for (field, value) in fields(header)? {
        match field {
            2 | 3 => {
                for (inner, bytes) in fields(value)? {
                    if inner == 1 {
                        keys.push(bytes.to_vec());
                        break;
                    }
                }
            }
            10000 => {
                for (inner, bytes) in fields(value)? {
                    if inner == 1 {
                        crx_id = Some(bytes);
                        break;
                    }
                }
            }
            _ => {}
        }
    }
    let crx_id = crx_id.ok_or(CrxError::Header("no signed extension id"))?;
    if crx_id.len() != 16 {
        return Err(CrxError::Header("the extension id is not 16 bytes"));
    }
    let id = id_from_bytes(crx_id);
    let public_key = keys
        .into_iter()
        .find(|key| id_for_key(key) == id)
        .ok_or(CrxError::Header("no key matches the signed extension id"))?;
    Ok((id, public_key))
}

/// The id Chromium derives from a public key (DER): its SHA-256, first
/// 16 bytes, in the id alphabet.
pub(crate) fn id_for_key(public_key: &[u8]) -> String {
    let digest = Sha256::digest(public_key);
    id_from_bytes(&digest[..16])
}

/// Chromium writes an id as the hex digits of 16 bytes, each digit moved
/// to the letters a to p.
pub(crate) fn id_from_bytes(bytes: &[u8]) -> String {
    bytes
        .iter()
        .flat_map(|b| [b >> 4, b & 0x0f])
        .map(|nibble| (b'a' + nibble) as char)
        .collect()
}

/// The length-delimited fields of one protobuf message, as (field number,
/// bytes). Varint and fixed-size fields are skipped.
fn fields(mut bytes: &[u8]) -> Result<Vec<(u64, &[u8])>, CrxError> {
    let mut out = Vec::new();
    while !bytes.is_empty() {
        let (tag, rest) = varint(bytes)?;
        bytes = rest;
        let field = tag >> 3;
        match tag & 7 {
            0 => {
                let (_, rest) = varint(bytes)?;
                bytes = rest;
            }
            1 => bytes = bytes.get(8..).ok_or(CrxError::Header("short fixed64"))?,
            2 => {
                let (len, rest) = varint(bytes)?;
                let len = usize::try_from(len).map_err(|_| CrxError::Header("length"))?;
                let value = rest
                    .get(..len)
                    .ok_or(CrxError::Header("field longer than the message"))?;
                out.push((field, value));
                bytes = &rest[len..];
            }
            5 => bytes = bytes.get(4..).ok_or(CrxError::Header("short fixed32"))?,
            _ => return Err(CrxError::Header("unknown wire type")),
        }
    }
    Ok(out)
}

fn varint(bytes: &[u8]) -> Result<(u64, &[u8]), CrxError> {
    let mut value = 0u64;
    for (i, byte) in bytes.iter().enumerate() {
        if i >= 10 {
            break;
        }
        value |= u64::from(byte & 0x7f) << (7 * i);
        if byte & 0x80 == 0 {
            return Ok((value, &bytes[i + 1..]));
        }
    }
    Err(CrxError::Header("bad varint"))
}

/// Standard base64 with padding, for the `key` field of a manifest.
pub(crate) fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        out.push(ALPHABET[(n >> 18) as usize & 63] as char);
        out.push(ALPHABET[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            ALPHABET[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            ALPHABET[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

/// True for a store extension id: 32 letters from a to p.
pub(crate) fn is_extension_id(text: &str) -> bool {
    text.len() == 32 && text.bytes().all(|b| (b'a'..=b'p').contains(&b))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A length-delimited protobuf field.
    fn field(number: u64, value: &[u8]) -> Vec<u8> {
        let mut out = encode_varint((number << 3) | 2);
        out.extend(encode_varint(value.len() as u64));
        out.extend_from_slice(value);
        out
    }

    fn encode_varint(mut v: u64) -> Vec<u8> {
        let mut out = Vec::new();
        loop {
            let byte = (v & 0x7f) as u8;
            v >>= 7;
            if v == 0 {
                out.push(byte);
                return out;
            }
            out.push(byte | 0x80);
        }
    }

    fn crx3(header: &[u8], archive: &[u8]) -> Vec<u8> {
        let mut out = MAGIC.to_vec();
        out.extend_from_slice(&3u32.to_le_bytes());
        out.extend_from_slice(&(header.len() as u32).to_le_bytes());
        out.extend_from_slice(header);
        out.extend_from_slice(archive);
        out
    }

    /// The signed id bytes for `key`.
    fn id_bytes(key: &[u8]) -> Vec<u8> {
        Sha256::digest(key)[..16].to_vec()
    }

    #[test]
    fn parses_the_id_key_and_archive_of_a_crx3() {
        let store_key = b"the store's signing key";
        let key = b"not really a DER key";
        let mut store_proof = field(1, store_key);
        store_proof.extend(field(2, b"signature"));
        let mut proof = field(1, key);
        proof.extend(field(2, b"signature"));
        let mut header = Vec::new();
        // A varint field first, to be skipped; the store's proof first,
        // as the store writes it.
        header.extend(encode_varint(4 << 3));
        header.extend(encode_varint(7));
        header.extend(field(2, &store_proof));
        header.extend(field(2, &proof));
        header.extend(field(10000, &field(1, &id_bytes(key))));
        let bytes = crx3(&header, b"PK\x03\x04zip");
        let crx = parse(&bytes).unwrap();
        assert_eq!(crx.id, id_for_key(key));
        assert_eq!(crx.public_key, key);
        assert_eq!(crx.archive, b"PK\x03\x04zip");
    }

    #[test]
    fn a_header_whose_keys_do_not_match_the_id_is_an_error() {
        let header = [
            field(2, &field(1, b"some key")),
            field(10000, &field(1, &[7u8; 16])),
        ]
        .concat();
        assert_eq!(
            parse(&crx3(&header, b"")).unwrap_err(),
            CrxError::Header("no key matches the signed extension id")
        );
    }

    #[test]
    fn rejects_other_formats_and_versions() {
        assert_eq!(parse(b"PK\x03\x04").unwrap_err(), CrxError::NotCrx);
        assert_eq!(parse(b"Cr24\x02").unwrap_err(), CrxError::Truncated);
        let mut v2 = MAGIC.to_vec();
        v2.extend_from_slice(&2u32.to_le_bytes());
        v2.extend_from_slice(&0u32.to_le_bytes());
        assert_eq!(parse(&v2).unwrap_err(), CrxError::Version(2));
        let short = crx3(&field(10000, &field(1, &[0u8; 16])), b"")[..20].to_vec();
        assert_eq!(parse(&short).unwrap_err(), CrxError::Truncated);
    }

    #[test]
    fn a_header_without_id_or_key_is_an_error() {
        let no_id = crx3(&field(2, &field(1, b"key")), b"");
        assert!(matches!(parse(&no_id).unwrap_err(), CrxError::Header(_)));
        let no_key = crx3(&field(10000, &field(1, &[0u8; 16])), b"");
        assert!(matches!(parse(&no_key).unwrap_err(), CrxError::Header(_)));
    }

    #[test]
    fn ids_use_the_a_to_p_alphabet() {
        assert_eq!(id_from_bytes(&[0x00, 0xff, 0x4a]), "aappek");
        assert_eq!(id_for_key(b"").len(), 32);
        assert!(is_extension_id("kgbhhnicmcdeehinjjcncccelhaomnae"));
        assert!(!is_extension_id("kgbhhnicmcdeehinjjcncccelhaomna"));
        assert!(!is_extension_id("kgbhhnicmcdeehinjjcncccelhaomnaz"));
    }

    #[test]
    fn base64_matches_the_standard_alphabet_and_padding() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
    }
}
