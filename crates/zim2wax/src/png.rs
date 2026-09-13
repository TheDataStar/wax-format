//! A dependency-free PNG encoder for the placeholder icon (§20: "falling back
//! to a generated placeholder, never omitted").
//!
//! Emits an 8-bit RGBA image with stored (uncompressed) deflate blocks. The
//! icon is 48×48 to match `Illustration_48x48@1`, the size every ZIM scraper
//! writes; a placeholder is by definition not worth compressing.

const CRC_TABLE: [u32; 256] = {
    let mut t = [0u32; 256];
    let mut n = 0;
    while n < 256 {
        let mut c = n as u32;
        let mut k = 0;
        while k < 8 {
            c = if c & 1 != 0 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 };
            k += 1;
        }
        t[n] = c;
        n += 1;
    }
    t
};

fn crc32(data: &[u8]) -> u32 {
    let mut c = 0xFFFF_FFFFu32;
    for &b in data {
        c = CRC_TABLE[((c ^ b as u32) & 0xFF) as usize] ^ (c >> 8);
    }
    c ^ 0xFFFF_FFFF
}

fn adler32(data: &[u8]) -> u32 {
    let (mut a, mut b) = (1u32, 0u32);
    for &x in data {
        a = (a + x as u32) % 65521;
        b = (b + a) % 65521;
    }
    (b << 16) | a
}

fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], body: &[u8]) {
    out.extend_from_slice(&(body.len() as u32).to_be_bytes());
    let mut crc_input = Vec::with_capacity(4 + body.len());
    crc_input.extend_from_slice(kind);
    crc_input.extend_from_slice(body);
    out.extend_from_slice(&crc_input);
    out.extend_from_slice(&crc32(&crc_input).to_be_bytes());
}

/// zlib stream with stored blocks only.
fn zlib_stored(raw: &[u8]) -> Vec<u8> {
    let mut z = vec![0x78, 0x01];
    let mut chunks = raw.chunks(65535).peekable();
    if raw.is_empty() {
        z.extend_from_slice(&[0x01, 0x00, 0x00, 0xFF, 0xFF]);
    }
    while let Some(c) = chunks.next() {
        let last = chunks.peek().is_none();
        z.push(if last { 0x01 } else { 0x00 });
        let len = c.len() as u16;
        z.extend_from_slice(&len.to_le_bytes());
        z.extend_from_slice(&(!len).to_le_bytes());
        z.extend_from_slice(c);
    }
    z.extend_from_slice(&adler32(raw).to_be_bytes());
    z
}

/// Encode an RGBA buffer (`w*h*4` bytes, row-major) as a PNG.
pub fn encode_rgba(w: u32, h: u32, rgba: &[u8]) -> Vec<u8> {
    assert_eq!(rgba.len(), (w * h * 4) as usize);
    let mut png = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    let mut ihdr = Vec::with_capacity(13);
    ihdr.extend_from_slice(&w.to_be_bytes());
    ihdr.extend_from_slice(&h.to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]); // 8-bit, RGBA, deflate, no filter, no interlace
    chunk(&mut png, b"IHDR", &ihdr);
    // filter byte 0 (None) before every row
    let stride = (w * 4) as usize;
    let mut raw = Vec::with_capacity((stride + 1) * h as usize);
    for row in rgba.chunks(stride) {
        raw.push(0);
        raw.extend_from_slice(row);
    }
    chunk(&mut png, b"IDAT", &zlib_stored(&raw));
    chunk(&mut png, b"IEND", &[]);
    png
}

/// The placeholder icon: a 48×48 slate-blue square with a lighter inner square,
/// so it is recognisably "no illustration supplied" rather than a broken image.
pub fn placeholder_icon() -> Vec<u8> {
    const W: u32 = 48;
    let mut rgba = vec![0u8; (W * W * 4) as usize];
    for y in 0..W {
        for x in 0..W {
            let inner = (8..40).contains(&x) && (8..40).contains(&y);
            let (r, g, b) = if inner { (0x9C, 0xB4, 0xD6) } else { (0x3E, 0x5C, 0x87) };
            let i = ((y * W + x) * 4) as usize;
            rgba[i..i + 4].copy_from_slice(&[r, g, b, 0xFF]);
        }
    }
    encode_rgba(W, W, &rgba)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn placeholder_is_a_structurally_valid_png() {
        let png = placeholder_icon();
        assert_eq!(&png[..8], &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]);
        // IHDR length 13, then type
        assert_eq!(&png[8..16], &[0, 0, 0, 13, b'I', b'H', b'D', b'R']);
        assert_eq!(u32::from_be_bytes(png[16..20].try_into().unwrap()), 48);
        assert_eq!(u32::from_be_bytes(png[20..24].try_into().unwrap()), 48);
        assert!(png.ends_with(&[b'I', b'E', b'N', b'D', 0xAE, 0x42, 0x60, 0x82]), "IEND crc");
        // every chunk's CRC checks out
        let mut i = 8;
        while i < png.len() {
            let len = u32::from_be_bytes(png[i..i + 4].try_into().unwrap()) as usize;
            let body = &png[i + 4..i + 8 + len];
            let crc = u32::from_be_bytes(png[i + 8 + len..i + 12 + len].try_into().unwrap());
            assert_eq!(crc32(body), crc, "chunk crc at {i}");
            i += 12 + len;
        }
    }

    #[test]
    fn crc32_known_answer() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(adler32(b"Wikipedia"), 0x11E6_0398);
    }
}
