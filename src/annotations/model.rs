use anyhow::{ensure, Result};
use serde::{de, Deserialize, Deserializer, Serialize};
use std::fmt;

pub const MAX_POINTS: usize = 200_000;
pub const MAX_STROKES: usize = 10_000;
pub const MAX_BYTES: usize = 8 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Point {
    pub x: f32,
    pub y: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum InkTool {
    Pen,
    Highlighter,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Stroke {
    pub tool: InkTool,
    pub color: [u8; 4],
    pub width: f32,
    #[serde(deserialize_with = "points")]
    pub points: Vec<Point>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PageInk {
    pub version: u32,
    #[serde(deserialize_with = "hash_text")]
    pub score_hash: String,
    pub page: u32,
    #[serde(deserialize_with = "strokes")]
    pub strokes: Vec<Stroke>,
}

fn bounded<'de, D, T, const N: usize>(deserializer: D) -> std::result::Result<Vec<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    struct Bounded<T, const N: usize>(std::marker::PhantomData<T>);
    impl<'de, T: Deserialize<'de>, const N: usize> de::Visitor<'de> for Bounded<T, N> {
        type Value = Vec<T>;
        fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(f, "an array of at most {N} elements")
        }
        fn visit_seq<A: de::SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
            let mut result = Vec::new();
            while let Some(value) = seq.next_element()? {
                if result.len() == N {
                    return Err(de::Error::custom("annotation array limit exceeded"));
                }
                result.push(value);
            }
            Ok(result)
        }
    }
    deserializer.deserialize_seq(Bounded::<T, N>(std::marker::PhantomData))
}

fn points<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<Point>, D::Error> {
    bounded::<D, Point, MAX_POINTS>(d)
}

fn strokes<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<Stroke>, D::Error> {
    struct Strokes;
    impl<'de> de::Visitor<'de> for Strokes {
        type Value = Vec<Stroke>;
        fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("a bounded array of strokes")
        }
        fn visit_seq<A: de::SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
            let mut result = Vec::new();
            let mut count = 0usize;
            while let Some(stroke) = seq.next_element::<Stroke>()? {
                count = count.saturating_add(stroke.points.len());
                if result.len() == MAX_STROKES || count > MAX_POINTS {
                    return Err(de::Error::custom("annotation stroke/point limit exceeded"));
                }
                result.push(stroke);
            }
            Ok(result)
        }
    }
    d.deserialize_seq(Strokes)
}

fn hash_text<'de, D: Deserializer<'de>>(d: D) -> Result<String, D::Error> {
    struct Hash;
    impl de::Visitor<'_> for Hash {
        type Value = String;
        fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("64 ASCII hexadecimal characters")
        }
        fn visit_str<E: de::Error>(self, text: &str) -> Result<String, E> {
            if !valid_hash(text) {
                return Err(E::custom("invalid score hash"));
            }
            Ok(text.to_owned())
        }
    }
    d.deserialize_str(Hash)
}

pub(crate) fn valid_hash(hash: &str) -> bool {
    hash.len() == 64 && hash.bytes().all(|c| c.is_ascii_hexdigit())
}

impl PageInk {
    pub fn empty(score_hash: &str, page: u32) -> Self {
        Self {
            version: 1,
            score_hash: score_hash.to_owned(),
            page,
            strokes: Vec::new(),
        }
    }

    pub fn validate(&self, pages: u32) -> Result<()> {
        ensure!(self.version == 1, "unsupported annotation schema");
        ensure!(valid_hash(&self.score_hash), "invalid score hash");
        ensure!(self.page < pages, "annotation page out of range");
        ensure!(self.strokes.len() <= MAX_STROKES, "too many strokes");
        let mut total = 0usize;
        for stroke in &self.strokes {
            ensure!(
                stroke.width.is_finite() && (0.0001..=0.1).contains(&stroke.width),
                "invalid stroke width"
            );
            ensure!(stroke.color[3] > 0, "transparent stroke");
            ensure!(!stroke.points.is_empty(), "empty stroke");
            total = total.saturating_add(stroke.points.len());
            ensure!(total <= MAX_POINTS, "too many points");
            for point in &stroke.points {
                ensure!(
                    point.x.is_finite()
                        && point.y.is_finite()
                        && (0.0..=1.0).contains(&point.x)
                        && (0.0..=1.0).contains(&point.y),
                    "invalid stroke coordinate"
                );
            }
        }
        Ok(())
    }
}

/// Test the eraser segment against the stroke's round-ended physical capsules.
/// Both radii are measured in page-width units; normalized y is aspect-scaled.
pub fn stroke_hit(
    stroke: &Stroke,
    a: Point,
    b: Point,
    radius: f32,
    page_height_over_width: f32,
) -> bool {
    if !radius.is_finite()
        || radius < 0.0
        || !page_height_over_width.is_finite()
        || page_height_over_width <= 0.0
        || !stroke.width.is_finite()
        || stroke.width < 0.0
    {
        return false;
    }
    let scale = |p: Point| {
        [
            f64::from(p.x),
            f64::from(p.y) * f64::from(page_height_over_width),
        ]
    };
    let a = scale(a);
    let b = scale(b);
    if !a.into_iter().chain(b).all(f64::is_finite) {
        return false;
    }
    let r2 = (f64::from(radius) + f64::from(stroke.width) / 2.0).powi(2);
    let hit = |p: Point, q: Point| {
        let p = scale(p);
        let q = scale(q);
        p.into_iter().chain(q).all(f64::is_finite) && segment_distance2(a, b, p, q) <= r2
    };
    match stroke.points.as_slice() {
        [] => false,
        [p] => hit(*p, *p),
        points => points.windows(2).any(|p| hit(p[0], p[1])),
    }
}

fn segment_distance2(a: [f64; 2], b: [f64; 2], c: [f64; 2], d: [f64; 2]) -> f64 {
    let sub = |p: [f64; 2], q: [f64; 2]| [p[0] - q[0], p[1] - q[1]];
    let cross = |p: [f64; 2], q: [f64; 2]| p[0] * q[1] - p[1] * q[0];
    let ab = sub(b, a);
    let cd = sub(d, c);
    let ac = sub(c, a);
    let denom = cross(ab, cd);
    if denom != 0.0 {
        let t = cross(ac, cd) / denom;
        let u = cross(ac, ab) / denom;
        if (0.0..=1.0).contains(&t) && (0.0..=1.0).contains(&u) {
            return 0.0;
        }
    }
    let point_segment = |p: [f64; 2], x: [f64; 2], y: [f64; 2]| {
        let v = sub(y, x);
        let w = sub(p, x);
        let len = v[0] * v[0] + v[1] * v[1];
        let t = if len == 0.0 {
            0.0
        } else {
            ((w[0] * v[0] + w[1] * v[1]) / len).clamp(0.0, 1.0)
        };
        (w[0] - t * v[0]).powi(2) + (w[1] - t * v[1]).powi(2)
    };
    point_segment(a, c, d)
        .min(point_segment(b, c, d))
        .min(point_segment(c, a, b))
        .min(point_segment(d, a, b))
}

#[cfg(test)]
mod tests {
    use super::*;
    const HASH: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    fn dot() -> Stroke {
        Stroke {
            tool: InkTool::Pen,
            color: [1, 2, 3, 255],
            width: 0.01,
            points: vec![Point { x: 0.5, y: 0.5 }],
        }
    }
    #[test]
    fn schema_identity_and_stable_serialization() {
        let ink = PageInk::empty(HASH, 0);
        assert!(ink.validate(1).is_ok());
        assert_eq!(
            serde_json::to_string(&ink).unwrap(),
            format!(r#"{{"version":1,"score_hash":"{HASH}","page":0,"strokes":[]}}"#)
        );
        for text in [
            format!(r#"{{"version":1,"score_hash":"{HASH}","page":0,"strokes":[],"extra":1}}"#),
            format!(
                r#"{{"version":1,"score_hash":"{HASH}","page":0,"strokes":[{{"tool":"Future","color":[1,2,3,4],"width":0.01,"points":[]}}]}}"#
            ),
        ] {
            assert!(serde_json::from_str::<PageInk>(&text).is_err());
        }
        for (hash, page, version) in [("bad", 0, 1), (HASH, 1, 1), (HASH, 0, 2)] {
            let mut invalid = PageInk::empty(hash, page);
            invalid.version = version;
            assert!(invalid.validate(1).is_err());
        }
    }
    #[test]
    fn validates_coordinates_width_alpha_and_limits() {
        let mut ink = PageInk::empty(HASH, 0);
        ink.strokes.push(dot());
        for x in [f32::NAN, f32::INFINITY, -0.1, 1.1] {
            ink.strokes[0].points[0].x = x;
            assert!(ink.validate(1).is_err());
        }
        ink.strokes[0] = dot();
        for width in [f32::NAN, 0.0, 0.1001] {
            ink.strokes[0].width = width;
            assert!(ink.validate(1).is_err());
        }
        ink.strokes[0] = dot();
        ink.strokes[0].color[3] = 0;
        assert!(ink.validate(1).is_err());
        ink.strokes[0] = dot();
        ink.strokes[0].points = vec![Point { x: 0.0, y: 0.0 }; MAX_POINTS];
        assert!(ink.validate(1).is_ok());
        ink.strokes[0].points.push(Point { x: 0.0, y: 0.0 });
        assert!(ink.validate(1).is_err());
        assert!(serde_json::from_slice::<PageInk>(&serde_json::to_vec(&ink).unwrap()).is_err());
        ink.strokes = vec![dot(); MAX_STROKES + 1];
        assert!(ink.validate(1).is_err());
        assert!(serde_json::from_slice::<PageInk>(&serde_json::to_vec(&ink).unwrap()).is_err());
    }
    #[test]
    fn capsules_crossings_dots_aspect_and_parallel_segments() {
        let mut stroke = dot();
        assert!(stroke_hit(
            &stroke,
            Point { x: 0.0, y: 0.5 },
            Point { x: 1.0, y: 0.5 },
            0.001,
            2.0
        ));
        assert!(!stroke_hit(
            &stroke,
            Point { x: 0.5, y: 0.51 },
            Point { x: 0.5, y: 0.51 },
            0.01,
            2.0
        ));
        assert!(stroke_hit(
            &stroke,
            Point { x: 0.51, y: 0.5 },
            Point { x: 0.51, y: 0.5 },
            0.01,
            2.0
        ));
        stroke.points = vec![Point { x: 0.1, y: 0.1 }, Point { x: 0.9, y: 0.9 }];
        assert!(stroke_hit(
            &stroke,
            Point { x: 0.1, y: 0.9 },
            Point { x: 0.9, y: 0.1 },
            0.0,
            1.4
        ));
        assert!(stroke_hit(
            &stroke,
            Point { x: 0.4, y: 0.4 },
            Point { x: 0.6, y: 0.6 },
            0.0,
            1.0
        ));
        assert!(!stroke_hit(
            &stroke,
            Point { x: 0.1, y: 0.2 },
            Point { x: 0.8, y: 0.9 },
            0.001,
            1.0
        ));
        assert!(!stroke_hit(
            &stroke,
            Point { x: 0.0, y: 0.0 },
            Point { x: 1.0, y: 1.0 },
            f32::NAN,
            1.0
        ));
    }
}
