//! Bounded, data-only snapshots of a parsed route. No filesystem or clipboard I/O.
use crate::route_preview::{ElevationStats, Route, Timing};
use serde::{Serialize, Serializer, ser::SerializeSeq};
use std::io::{self, Write};
const LIMIT: usize = 1024 * 1024;
struct Bounded(Vec<u8>);
impl Write for Bounded {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() > LIMIT.saturating_sub(self.0.len()) {
            return Err(io::Error::other("Route JSON reports are limited to 1 MiB"));
        }
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
#[derive(Serialize)]
struct Point {
    index: usize,
    segment: usize,
    start: bool,
    lat: f64,
    lon: f64,
    elevation_m: Option<f64>,
    time: Option<String>,
    time_invalid: bool,
    distance_m: f64,
}
struct Points<'a>(&'a Route);
impl Serialize for Points<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut rows = serializer.serialize_seq(Some(self.0.points.len()))?;
        let mut segment = 0;
        for (index, p) in self.0.points.iter().enumerate() {
            segment += usize::from(p.start);
            rows.serialize_element(&Point {
                index,
                segment,
                start: p.start,
                lat: p.lat,
                lon: p.lon,
                elevation_m: p.elevation,
                time: p.time.map(|t| t.to_rfc3339()),
                time_invalid: p.invalid_time,
                distance_m: p.distance,
            })?;
        }
        rows.end()
    }
}
#[derive(Serialize)]
struct Report<'a> {
    format: &'static str,
    version: u8,
    source_format: &'static str,
    distance_model: &'static str,
    earth_radius_m: f64,
    gap_policy: &'static str,
    elevation_model: &'static str,
    distance_m: f64,
    segments: usize,
    timing: Timing,
    elevation_stats: ElevationStats,
    points: Points<'a>,
}
pub fn json(route: &Route) -> Result<String, String> {
    if route.points.is_empty()
        || route.points.len() > 10_000
        || !route.distance.is_finite()
        || route.points.iter().any(|p| {
            !p.lat.is_finite()
                || !p.lon.is_finite()
                || !p.distance.is_finite()
                || p.elevation.is_some_and(|e| !e.is_finite())
        })
    {
        return Err("Route report needs a bounded finite parsed route".into());
    }
    let report = Report {
        format: "commander.route-preview",
        version: 1,
        source_format: route.format,
        distance_model: "spherical_haversine",
        earth_radius_m: 6_371_008.8,
        gap_policy: "no_interpolation",
        elevation_model: "raw_adjacent_samples",
        distance_m: route.distance,
        segments: route.segments,
        timing: route.timing(),
        elevation_stats: route.elevation_stats(),
        points: Points(route),
    };
    let mut writer = Bounded(Vec::new());
    serde_json::to_writer_pretty(&mut writer, &report).map_err(|e| e.to_string())?;
    String::from_utf8(writer.0).map_err(|e| e.to_string())
}
#[cfg(test)]
mod tests {
    use super::*;
    fn wire(value: impl Serialize) -> serde_json::Value {
        serde_json::from_slice(&serde_json::to_vec(&value).unwrap()).unwrap()
    }
    #[test]
    fn report_preserves_points_segments_times_and_unknown_statistics_without_mutation() {
        for text in [
            crate::route_preview::fixture(),
            crate::route_preview::kml_fixture(),
            r#"<gpx><rte><rtept lat="1" lon="2"/></rte></gpx>"#,
        ] {
            let route = crate::route_preview::parse(text, &|| false).unwrap();
            let before = route.clone();
            let value: serde_json::Value = serde_json::from_str(&json(&route).unwrap()).unwrap();
            assert_eq!(value["format"], "commander.route-preview");
            assert_eq!(value["version"], 1);
            assert_eq!(value["source_format"], route.format);
            assert_eq!(value["distance_m"], wire(route.distance));
            assert_eq!(value["timing"], wire(route.timing()));
            assert_eq!(value["elevation_stats"], wire(route.elevation_stats()));
            let mut segment = 0;
            for (index, (row, p)) in value["points"]
                .as_array()
                .unwrap()
                .iter()
                .zip(&route.points)
                .enumerate()
            {
                segment += usize::from(p.start);
                assert_eq!(row["index"], index);
                assert_eq!(row["segment"], segment);
                assert_eq!(row["lat"], wire(p.lat));
                assert_eq!(row["lon"], wire(p.lon));
                assert_eq!(row["distance_m"], wire(p.distance));
                assert_eq!(row["elevation_m"], wire(p.elevation));
                assert_eq!(row["time"], wire(p.time.map(|t| t.to_rfc3339())));
            }
            assert_eq!(route, before);
        }
    }
    #[test]
    fn report_refuses_nonfinite_and_oversized_output_before_publication() {
        let mut route =
            crate::route_preview::parse(crate::route_preview::fixture(), &|| false).unwrap();
        route.distance = f64::NAN;
        assert!(json(&route).is_err());
        route.distance = 0.;
        route.points = vec![route.points[0].clone(); 10_000];
        assert!(json(&route).unwrap_err().contains("1 MiB"));
        route.points.push(route.points[0].clone());
        assert!(json(&route).is_err());
    }
}
