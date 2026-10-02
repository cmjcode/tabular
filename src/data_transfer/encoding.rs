//! Encoding teks untuk ekspor (pilihan encoding + BOM) dan impor (deteksi
//! UTF-8 / UTF-16 / Windows-1252).

/// Encoding teks yang didukung ekspor dan impor.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TextEncoding {
    #[default]
    Utf8,
    Utf16Le,
    Utf16Be,
    Windows1252,
}

impl TextEncoding {
    pub const ALL: [TextEncoding; 4] = [
        TextEncoding::Utf8,
        TextEncoding::Utf16Le,
        TextEncoding::Utf16Be,
        TextEncoding::Windows1252,
    ];

    pub fn label(self) -> &'static str {
        match self {
            TextEncoding::Utf8 => "UTF-8",
            TextEncoding::Utf16Le => "UTF-16 LE",
            TextEncoding::Utf16Be => "UTF-16 BE",
            TextEncoding::Windows1252 => "Windows-1252",
        }
    }

    /// Nama untuk deklarasi `<?xml encoding=...?>` dan `<meta charset>`.
    pub fn iana_name(self) -> &'static str {
        match self {
            TextEncoding::Utf8 => "UTF-8",
            TextEncoding::Utf16Le | TextEncoding::Utf16Be => "UTF-16",
            TextEncoding::Windows1252 => "windows-1252",
        }
    }

    fn bom(self) -> &'static [u8] {
        match self {
            TextEncoding::Utf8 => &[0xEF, 0xBB, 0xBF],
            TextEncoding::Utf16Le => &[0xFF, 0xFE],
            TextEncoding::Utf16Be => &[0xFE, 0xFF],
            TextEncoding::Windows1252 => &[],
        }
    }
}

/// Hasil encode: byte + jumlah karakter yang tidak punya padanan di encoding
/// tujuan (hanya mungkin untuk Windows-1252; diganti `?`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Encoded {
    pub bytes: Vec<u8>,
    pub unmappable: usize,
}

/// Encode `text`. `bom` menambahkan byte order mark (diabaikan untuk
/// Windows-1252 yang tidak punya BOM).
pub fn encode(text: &str, encoding: TextEncoding, bom: bool) -> Encoded {
    let mut bytes = Vec::with_capacity(text.len() + 3);
    if bom {
        bytes.extend_from_slice(encoding.bom());
    }
    let mut unmappable = 0;
    match encoding {
        TextEncoding::Utf8 => bytes.extend_from_slice(text.as_bytes()),
        TextEncoding::Utf16Le => {
            for unit in text.encode_utf16() {
                bytes.extend_from_slice(&unit.to_le_bytes());
            }
        }
        TextEncoding::Utf16Be => {
            for unit in text.encode_utf16() {
                bytes.extend_from_slice(&unit.to_be_bytes());
            }
        }
        TextEncoding::Windows1252 => {
            // `Encoding::encode` mengganti karakter tak terpetakan dengan
            // referensi numerik HTML; untuk file data lebih aman `?`.
            let mut encoder = encoding_rs::WINDOWS_1252.new_encoder();
            let mut buf = [0u8; 8];
            let mut utf8 = [0u8; 4];
            for ch in text.chars() {
                let src = ch.encode_utf8(&mut utf8);
                let (result, _read, written) =
                    encoder.encode_from_utf8_without_replacement(src, &mut buf, false);
                match result {
                    encoding_rs::EncoderResult::InputEmpty => {
                        bytes.extend_from_slice(&buf[..written])
                    }
                    _ => {
                        bytes.push(b'?');
                        unmappable += 1;
                    }
                }
            }
        }
    }
    Encoded { bytes, unmappable }
}

/// Tebak encoding dari isi: BOM dulu, lalu pola byte nol khas UTF-16 tanpa
/// BOM, lalu UTF-8 valid, terakhir Windows-1252.
pub fn detect(bytes: &[u8]) -> TextEncoding {
    if bytes.starts_with(&[0xEF, 0xBB, 0xBF]) {
        return TextEncoding::Utf8;
    }
    if bytes.starts_with(&[0xFF, 0xFE]) {
        return TextEncoding::Utf16Le;
    }
    if bytes.starts_with(&[0xFE, 0xFF]) {
        return TextEncoding::Utf16Be;
    }
    let sample = &bytes[..bytes.len().min(64 * 1024)];
    if sample.len() >= 4 {
        let pairs = sample.len() / 2;
        let even_zero = sample.iter().step_by(2).filter(|b| **b == 0).count();
        let odd_zero = sample
            .iter()
            .skip(1)
            .step_by(2)
            .filter(|b| **b == 0)
            .count();
        // Teks Latin dalam UTF-16: hampir separuh byte bernilai nol di satu sisi.
        if odd_zero * 10 >= pairs * 6 && even_zero * 10 <= pairs {
            return TextEncoding::Utf16Le;
        }
        if even_zero * 10 >= pairs * 6 && odd_zero * 10 <= pairs {
            return TextEncoding::Utf16Be;
        }
    }
    // Potongan sampel bisa memutus karakter multi-byte di ujung; itu tetap UTF-8.
    match std::str::from_utf8(sample) {
        Ok(_) => TextEncoding::Utf8,
        Err(e) if e.error_len().is_none() && sample.len() < bytes.len() => TextEncoding::Utf8,
        Err(_) => TextEncoding::Windows1252,
    }
}

/// Decode `bytes` jadi teks. `hint = None` berarti deteksi otomatis. BOM
/// dibuang. Mengembalikan teks dan encoding yang dipakai.
pub fn decode(bytes: &[u8], hint: Option<TextEncoding>) -> (String, TextEncoding) {
    let encoding = hint.unwrap_or_else(|| detect(bytes));
    let body = bytes.strip_prefix(encoding.bom()).unwrap_or(bytes);
    let text = match encoding {
        TextEncoding::Utf8 => String::from_utf8_lossy(body).into_owned(),
        TextEncoding::Utf16Le => encoding_rs::UTF_16LE
            .decode_without_bom_handling(body)
            .0
            .into_owned(),
        TextEncoding::Utf16Be => encoding_rs::UTF_16BE
            .decode_without_bom_handling(body)
            .0
            .into_owned(),
        TextEncoding::Windows1252 => encoding_rs::WINDOWS_1252
            .decode_without_bom_handling(body)
            .0
            .into_owned(),
    };
    (text, encoding)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf16_roundtrip_with_and_without_bom() {
        let text = "id,name\n1,Żółć\n";
        for enc in [TextEncoding::Utf16Le, TextEncoding::Utf16Be] {
            for bom in [true, false] {
                let encoded = encode(text, enc, bom);
                assert_eq!(encoded.unmappable, 0);
                let (decoded, used) = decode(&encoded.bytes, Some(enc));
                assert_eq!(decoded, text);
                assert_eq!(used, enc);
            }
        }
    }

    #[test]
    fn detects_bom_and_bomless_utf16() {
        let text = "id,name\n1,alice\n2,bob\n";
        assert_eq!(
            detect(&encode(text, TextEncoding::Utf8, true).bytes),
            TextEncoding::Utf8
        );
        assert_eq!(
            detect(&encode(text, TextEncoding::Utf16Le, false).bytes),
            TextEncoding::Utf16Le
        );
        assert_eq!(
            detect(&encode(text, TextEncoding::Utf16Be, false).bytes),
            TextEncoding::Utf16Be
        );
        assert_eq!(detect(text.as_bytes()), TextEncoding::Utf8);
    }

    #[test]
    fn windows_1252_roundtrip_and_unmappable() {
        let encoded = encode("café €5", TextEncoding::Windows1252, true);
        assert_eq!(encoded.unmappable, 0);
        // 0xE9 = é, 0x80 = € di Windows-1252; bukan UTF-8 yang valid.
        assert!(encoded.bytes.contains(&0xE9) && encoded.bytes.contains(&0x80));
        assert_eq!(detect(&encoded.bytes), TextEncoding::Windows1252);
        assert_eq!(decode(&encoded.bytes, None).0, "café €5");

        let lossy = encode("日本", TextEncoding::Windows1252, false);
        assert_eq!(lossy.unmappable, 2);
        assert_eq!(lossy.bytes, b"??");
    }

    #[test]
    fn decode_strips_utf8_bom() {
        let (text, enc) = decode(&[0xEF, 0xBB, 0xBF, b'a'], None);
        assert_eq!(text, "a");
        assert_eq!(enc, TextEncoding::Utf8);
    }
}
