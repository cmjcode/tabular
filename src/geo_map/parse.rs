//! Parser geometry dari nilai sel hasil query (checklist C4).
//!
//! Format yang dikenali:
//! - WKT/EWKT: `POINT(1 2)`, `SRID=4326;POLYGON((...))`, termasuk Z/M/ZM dan `EMPTY`.
//! - GeoJSON: Geometry, Feature, FeatureCollection (hasil `ST_AsGeoJSON`).
//! - WKB/EWKB hex: kolom `geometry` PostGIS tampil sebagai `\x0101000020E6...`.
//! - Format internal MySQL: 4 byte SRID little-endian + WKB, tampil sebagai `0x...`.

/// Titik [x, y]; untuk data geografis x = longitude, y = latitude.
pub type Coord = [f64; 2];

#[derive(Debug, Clone, PartialEq)]
pub enum Shape {
    Point(Coord),
    Line(Vec<Coord>),
    /// Ring pertama adalah batas luar, sisanya lubang.
    Polygon(Vec<Vec<Coord>>),
    Collection(Vec<Shape>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Geometry {
    pub srid: Option<i32>,
    pub shape: Shape,
}

impl Shape {
    /// Memanggil `f` untuk setiap koordinat.
    pub fn for_each_coord(&self, f: &mut impl FnMut(Coord)) {
        match self {
            Shape::Point(c) => f(*c),
            Shape::Line(cs) => cs.iter().for_each(|c| f(*c)),
            Shape::Polygon(rings) => rings.iter().flatten().for_each(|c| f(*c)),
            Shape::Collection(items) => items.iter().for_each(|s| s.for_each_coord(f)),
        }
    }

    /// Mengubah setiap koordinat di tempat.
    pub fn map_coords(&mut self, f: &impl Fn(Coord) -> Coord) {
        match self {
            Shape::Point(c) => *c = f(*c),
            Shape::Line(cs) => cs.iter_mut().for_each(|c| *c = f(*c)),
            Shape::Polygon(rings) => rings.iter_mut().flatten().for_each(|c| *c = f(*c)),
            Shape::Collection(items) => items.iter_mut().for_each(|s| s.map_coords(f)),
        }
    }

    fn is_empty(&self) -> bool {
        match self {
            Shape::Point(c) => !(c[0].is_finite() && c[1].is_finite()),
            Shape::Line(cs) => cs.is_empty(),
            Shape::Polygon(rings) => rings.iter().all(|r| r.is_empty()),
            Shape::Collection(items) => items.iter().all(|s| s.is_empty()),
        }
    }
}

const WKT_KEYWORDS: [&str; 7] = [
    "POINT",
    "LINESTRING",
    "POLYGON",
    "MULTIPOINT",
    "MULTILINESTRING",
    "MULTIPOLYGON",
    "GEOMETRYCOLLECTION",
];

/// Uji murah sebelum parsing penuh; dipanggil untuk banyak sel.
pub fn looks_like_geometry(v: &str) -> bool {
    let t = v.trim_start();
    if t.len() < 10 {
        return false;
    }
    let head: String = t.chars().take(20).collect::<String>().to_ascii_uppercase();
    if head.starts_with("SRID=") || WKT_KEYWORDS.iter().any(|k| head.starts_with(k)) {
        return true;
    }
    if t.starts_with('{') {
        return (t.contains("\"type\"") && t.contains("\"coordinates\""))
            || t.contains("\"geometry\"");
    }
    let hex = t
        .strip_prefix("\\x")
        .or_else(|| t.strip_prefix("0x"))
        .or_else(|| t.strip_prefix("0X"))
        .unwrap_or(t);
    // Titik WKB = 21 byte, tetapi MySQL memangkas NUL di akhir sehingga bisa lebih pendek.
    hex.len() >= 26 && hex.bytes().take(64).all(|b| b.is_ascii_hexdigit())
}

/// Mem-parse satu nilai sel menjadi geometry.
pub fn parse_geometry(v: &str) -> Option<Geometry> {
    let t = v.trim();
    if !looks_like_geometry(t) {
        return None;
    }
    let g = if t.starts_with('{') {
        parse_geojson(t)
    } else if (t.as_bytes()[0].is_ascii_alphabetic()
        && !t.as_bytes()[..2].iter().all(|b| b.is_ascii_hexdigit()))
        || t.to_ascii_uppercase().starts_with("SRID=")
    {
        parse_wkt(t)
    } else {
        parse_hex_wkb(t)
    }?;
    (!g.shape.is_empty()).then_some(g)
}

// ─────────────────────────────────────────────────────────────────────────────
// WKT
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Word(String),
    Num(f64),
    Open,
    Close,
    Comma,
}

fn tokenize(s: &str) -> Option<Vec<Tok>> {
    let mut out = Vec::new();
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        match c {
            b'(' => {
                out.push(Tok::Open);
                i += 1;
            }
            b')' => {
                out.push(Tok::Close);
                i += 1;
            }
            b',' => {
                out.push(Tok::Comma);
                i += 1;
            }
            c if c.is_ascii_whitespace() => i += 1,
            c if c.is_ascii_alphabetic() => {
                let start = i;
                while i < b.len() && b[i].is_ascii_alphabetic() {
                    i += 1;
                }
                out.push(Tok::Word(s[start..i].to_ascii_uppercase()));
            }
            c if c.is_ascii_digit() || c == b'-' || c == b'+' || c == b'.' => {
                let start = i;
                i += 1;
                while i < b.len()
                    && (b[i].is_ascii_digit()
                        || matches!(b[i], b'.' | b'e' | b'E')
                        || (matches!(b[i], b'-' | b'+') && matches!(b[i - 1], b'e' | b'E')))
                {
                    i += 1;
                }
                out.push(Tok::Num(s[start..i].parse().ok()?));
            }
            _ => return None,
        }
    }
    Some(out)
}

struct WktParser {
    toks: Vec<Tok>,
    pos: usize,
}

impl WktParser {
    fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.pos)
    }

    fn next(&mut self) -> Option<Tok> {
        let t = self.toks.get(self.pos).cloned();
        self.pos += 1;
        t
    }

    fn expect(&mut self, t: Tok) -> Option<()> {
        (self.next()? == t).then_some(())
    }

    fn number(&mut self) -> Option<f64> {
        match self.next()? {
            Tok::Num(n) => Some(n),
            _ => None,
        }
    }

    fn coord(&mut self) -> Option<Coord> {
        let x = self.number()?;
        let y = self.number()?;
        // Z dan M diabaikan.
        while matches!(self.peek(), Some(Tok::Num(_))) {
            self.pos += 1;
        }
        Some([x, y])
    }

    /// `( a , b , ... )` dengan `item` untuk tiap elemen.
    fn list<T>(&mut self, mut item: impl FnMut(&mut Self) -> Option<T>) -> Option<Vec<T>> {
        self.expect(Tok::Open)?;
        let mut out = vec![item(self)?];
        loop {
            match self.next()? {
                Tok::Comma => out.push(item(self)?),
                Tok::Close => return Some(out),
                _ => return None,
            }
        }
    }

    fn coords(&mut self) -> Option<Vec<Coord>> {
        self.list(|p| p.coord())
    }

    fn polygon_body(&mut self) -> Option<Vec<Vec<Coord>>> {
        self.list(|p| p.coords())
    }

    fn geometry(&mut self) -> Option<Shape> {
        let Tok::Word(kind) = self.next()? else {
            return None;
        };
        // Penanda dimensi opsional.
        if let Some(Tok::Word(w)) = self.peek()
            && matches!(w.as_str(), "Z" | "M" | "ZM")
        {
            self.pos += 1;
        }
        if let Some(Tok::Word(w)) = self.peek()
            && w == "EMPTY"
        {
            self.pos += 1;
            return Some(Shape::Collection(Vec::new()));
        }
        match kind.as_str() {
            "POINT" => {
                self.expect(Tok::Open)?;
                let c = self.coord()?;
                self.expect(Tok::Close)?;
                Some(Shape::Point(c))
            }
            "LINESTRING" => Some(Shape::Line(self.coords()?)),
            "POLYGON" => Some(Shape::Polygon(self.polygon_body()?)),
            "MULTIPOINT" => {
                let pts = self.list(|p| {
                    if matches!(p.peek(), Some(Tok::Open)) {
                        p.pos += 1;
                        let c = p.coord()?;
                        p.expect(Tok::Close)?;
                        Some(c)
                    } else {
                        p.coord()
                    }
                })?;
                Some(Shape::Collection(
                    pts.into_iter().map(Shape::Point).collect(),
                ))
            }
            "MULTILINESTRING" => Some(Shape::Collection(
                self.list(|p| p.coords())?
                    .into_iter()
                    .map(Shape::Line)
                    .collect(),
            )),
            "MULTIPOLYGON" => Some(Shape::Collection(
                self.list(|p| p.polygon_body())?
                    .into_iter()
                    .map(Shape::Polygon)
                    .collect(),
            )),
            "GEOMETRYCOLLECTION" => Some(Shape::Collection(self.list(|p| p.geometry())?)),
            _ => None,
        }
    }
}

pub fn parse_wkt(s: &str) -> Option<Geometry> {
    let mut body = s.trim();
    let mut srid = None;
    if body.len() > 5 && body[..5].eq_ignore_ascii_case("SRID=") {
        let semi = body.find(';')?;
        srid = body[5..semi].trim().parse().ok();
        body = body[semi + 1..].trim();
    }
    let mut p = WktParser {
        toks: tokenize(body)?,
        pos: 0,
    };
    let shape = p.geometry()?;
    (p.pos == p.toks.len()).then_some(Geometry { srid, shape })
}

// ─────────────────────────────────────────────────────────────────────────────
// GeoJSON
// ─────────────────────────────────────────────────────────────────────────────

fn json_coord(v: &serde_json::Value) -> Option<Coord> {
    let a = v.as_array()?;
    Some([a.first()?.as_f64()?, a.get(1)?.as_f64()?])
}

fn json_coords(v: &serde_json::Value) -> Option<Vec<Coord>> {
    v.as_array()?.iter().map(json_coord).collect()
}

fn json_rings(v: &serde_json::Value) -> Option<Vec<Vec<Coord>>> {
    v.as_array()?.iter().map(json_coords).collect()
}

fn json_shape(v: &serde_json::Value) -> Option<Shape> {
    let kind = v.get("type")?.as_str()?;
    let c = v.get("coordinates");
    match kind {
        "Point" => Some(Shape::Point(json_coord(c?)?)),
        "LineString" => Some(Shape::Line(json_coords(c?)?)),
        "Polygon" => Some(Shape::Polygon(json_rings(c?)?)),
        "MultiPoint" => Some(Shape::Collection(
            json_coords(c?)?.into_iter().map(Shape::Point).collect(),
        )),
        "MultiLineString" => Some(Shape::Collection(
            json_rings(c?)?.into_iter().map(Shape::Line).collect(),
        )),
        "MultiPolygon" => Some(Shape::Collection(
            c?.as_array()?
                .iter()
                .map(|p| json_rings(p).map(Shape::Polygon))
                .collect::<Option<Vec<_>>>()?,
        )),
        "GeometryCollection" => Some(Shape::Collection(
            v.get("geometries")?
                .as_array()?
                .iter()
                .map(json_shape)
                .collect::<Option<Vec<_>>>()?,
        )),
        "Feature" => json_shape(v.get("geometry")?),
        "FeatureCollection" => Some(Shape::Collection(
            v.get("features")?
                .as_array()?
                .iter()
                .filter_map(json_shape)
                .collect(),
        )),
        _ => None,
    }
}

pub fn parse_geojson(s: &str) -> Option<Geometry> {
    let v: serde_json::Value = serde_json::from_str(s).ok()?;
    // GeoJSON RFC 7946 selalu WGS84 (EPSG:4326).
    Some(Geometry {
        srid: Some(4326),
        shape: json_shape(&v)?,
    })
}

// ─────────────────────────────────────────────────────────────────────────────
// WKB / EWKB
// ─────────────────────────────────────────────────────────────────────────────

struct WkbReader<'a> {
    b: &'a [u8],
    pos: usize,
    /// Byte yang "dipinjam" dari luar buffer. MySQL memangkas NUL di akhir nilai biner,
    /// jadi koordinat terakhir bernilai 0 bisa hilang beberapa byte.
    padded: usize,
}

impl WkbReader<'_> {
    fn take<const N: usize>(&mut self) -> [u8; N] {
        let mut out = [0u8; N];
        for (i, slot) in out.iter_mut().enumerate() {
            match self.b.get(self.pos + i) {
                Some(v) => *slot = *v,
                None => self.padded += 1,
            }
        }
        self.pos += N;
        out
    }

    fn u8(&mut self) -> u8 {
        self.take::<1>()[0]
    }

    fn u32(&mut self, le: bool) -> u32 {
        let b = self.take::<4>();
        if le {
            u32::from_le_bytes(b)
        } else {
            u32::from_be_bytes(b)
        }
    }

    fn f64(&mut self, le: bool) -> f64 {
        let b = self.take::<8>();
        if le {
            f64::from_le_bytes(b)
        } else {
            f64::from_be_bytes(b)
        }
    }

    fn remaining(&self) -> usize {
        self.b.len().saturating_sub(self.pos)
    }

    fn geometry(&mut self, depth: usize, srid: &mut Option<i32>) -> Option<Shape> {
        if depth > 16 {
            return None;
        }
        let le = match self.u8() {
            0 => false,
            1 => true,
            _ => return None,
        };
        let raw_type = self.u32(le);
        let has_srid = raw_type & 0x2000_0000 != 0;
        let mut dims = 2;
        if raw_type & 0x8000_0000 != 0 {
            dims += 1;
        }
        if raw_type & 0x4000_0000 != 0 {
            dims += 1;
        }
        let iso = raw_type & 0x0FFF_FFFF;
        let base = iso % 1000;
        dims += match iso / 1000 {
            0 => 0,
            1 | 2 => 1,
            3 => 2,
            _ => return None,
        };
        if !(1..=7).contains(&base) {
            return None;
        }
        if has_srid {
            let s = self.u32(le) as i32;
            if srid.is_none() {
                *srid = Some(s);
            }
        }
        let point = |r: &mut Self| -> [f64; 2] {
            let x = r.f64(le);
            let y = r.f64(le);
            for _ in 2..dims {
                r.f64(le);
            }
            [x, y]
        };
        let bytes_per_point = dims * 8;
        // Batas jumlah elemen supaya data acak tidak memicu alokasi besar.
        let check = |r: &Self, n: u32, unit: usize| (n as usize) <= r.remaining() / unit + 1;
        match base {
            1 => Some(Shape::Point(point(self))),
            2 => {
                let n = self.u32(le);
                if !check(self, n, bytes_per_point) {
                    return None;
                }
                Some(Shape::Line((0..n).map(|_| point(self)).collect()))
            }
            3 => {
                let rings = self.u32(le);
                if !check(self, rings, 4) {
                    return None;
                }
                let mut out = Vec::with_capacity(rings as usize);
                for _ in 0..rings {
                    let n = self.u32(le);
                    if !check(self, n, bytes_per_point) {
                        return None;
                    }
                    out.push((0..n).map(|_| point(self)).collect());
                }
                Some(Shape::Polygon(out))
            }
            _ => {
                let n = self.u32(le);
                if !check(self, n, 5) {
                    return None;
                }
                let mut items = Vec::with_capacity(n as usize);
                for _ in 0..n {
                    items.push(self.geometry(depth + 1, srid)?);
                }
                Some(Shape::Collection(items))
            }
        }
    }
}

fn decode_hex(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok())
        .collect()
}

/// Mem-parse WKB biner. `srid_hint` dipakai bila WKB sendiri tidak membawa SRID.
pub fn parse_wkb(bytes: &[u8], srid_hint: Option<i32>) -> Option<Geometry> {
    let mut r = WkbReader {
        b: bytes,
        pos: 0,
        padded: 0,
    };
    let mut srid = None;
    let shape = r.geometry(0, &mut srid)?;
    // Byte sisa atau padding lebih dari satu koordinat menandakan bukan WKB.
    if r.pos < bytes.len() || r.padded > 16 {
        return None;
    }
    Some(Geometry {
        srid: srid.or(srid_hint),
        shape,
    })
}

pub fn parse_hex_wkb(s: &str) -> Option<Geometry> {
    let hex = s
        .strip_prefix("\\x")
        .or_else(|| s.strip_prefix("0x"))
        .or_else(|| s.strip_prefix("0X"))
        .unwrap_or(s);
    let bytes = decode_hex(hex)?;
    if let Some(g) = parse_wkb(&bytes, None) {
        return Some(g);
    }
    // Format internal MySQL: SRID u32 little-endian, lalu WKB.
    if bytes.len() > 4 {
        let srid = i32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
        return parse_wkb(&bytes[4..], Some(srid));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wkt_dasar_dan_ewkt() {
        let g = parse_geometry("SRID=4326;POINT(106.8 -6.2)").unwrap();
        assert_eq!(g.srid, Some(4326));
        assert_eq!(g.shape, Shape::Point([106.8, -6.2]));

        let g = parse_geometry("POLYGON Z ((0 0 1, 4 0 1, 4 4 1, 0 0 1))").unwrap();
        assert!(matches!(g.shape, Shape::Polygon(ref r) if r[0].len() == 4));

        let g = parse_geometry("MULTIPOINT((1 2),(3 4))").unwrap();
        assert_eq!(
            g.shape,
            Shape::Collection(vec![Shape::Point([1.0, 2.0]), Shape::Point([3.0, 4.0])])
        );
        let g = parse_geometry("MULTIPOINT(1 2, 3 4)").unwrap();
        assert!(matches!(g.shape, Shape::Collection(ref v) if v.len() == 2));

        let g =
            parse_geometry("GEOMETRYCOLLECTION(POINT(1 1),LINESTRING(0 0,1e1 -2.5E-1))").unwrap();
        assert!(matches!(g.shape, Shape::Collection(ref v) if v.len() == 2));
        assert!(parse_geometry("POINT EMPTY").is_none());
        assert!(parse_geometry("POINTLESS text here").is_none());
    }

    #[test]
    fn geojson_feature_dan_multipolygon() {
        let g = parse_geometry(
            r#"{"type":"Feature","properties":{},"geometry":{"type":"Point","coordinates":[10.5,20.25]}}"#,
        )
        .unwrap();
        assert_eq!(g.shape, Shape::Point([10.5, 20.25]));
        let g = parse_geometry(
            r#"{"type":"MultiPolygon","coordinates":[[[[0,0],[1,0],[1,1],[0,0]]]]}"#,
        )
        .unwrap();
        assert!(matches!(g.shape, Shape::Collection(ref v) if v.len() == 1));
    }

    #[test]
    fn ewkb_postgis_hex() {
        // ST_AsEWKB('SRID=4326;POINT(1 2)')
        let g = parse_geometry("\\x0101000020E6100000000000000000F03F0000000000000040").unwrap();
        assert_eq!(g.srid, Some(4326));
        assert_eq!(g.shape, Shape::Point([1.0, 2.0]));
        // WKB big-endian tanpa SRID: POINT(1 2)
        let g = parse_geometry("00000000013FF00000000000004000000000000000").unwrap();
        assert_eq!(g.shape, Shape::Point([1.0, 2.0]));
    }

    #[test]
    fn format_internal_mysql_dengan_nul_terpangkas() {
        // SRID 4326 + WKB POINT(1 0): byte terakhir (0.0) dipangkas driver.
        let full = concat!(
            "E6100000",         // SRID
            "01",               // little-endian
            "01000000",         // Point
            "000000000000F03F", // x = 1.0
            "0000000000000000", // y = 0.0
        );
        let trimmed = full.trim_end_matches('0');
        let g = parse_geometry(&format!("0x{trimmed}")).unwrap();
        assert_eq!(g.srid, Some(4326));
        assert_eq!(g.shape, Shape::Point([1.0, 0.0]));
    }

    #[test]
    fn menolak_nilai_yang_bukan_geometry() {
        assert!(parse_geometry("0x0123456789abcdef0123456789abcdef0123456789ab").is_none());
        assert!(parse_geometry("550e8400-e29b-41d4-a716-446655440000").is_none());
        assert!(parse_geometry(r#"{"type":"object","coordinates":1}"#).is_none());
        assert!(parse_geometry("hello world").is_none());
    }
}
