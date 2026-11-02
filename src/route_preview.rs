//! Offline GPX/KML display, independently implemented from the Strata route-preview idea.
use egui::{Color32, Pos2, Rect, Stroke, Vec2};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
const LIMIT: usize = 256 * 1024;
const POINT_LIMIT: usize = 10_000;
#[derive(Clone, Debug, PartialEq)]
pub struct Point {
    pub lat: f64,
    pub lon: f64,
    pub elevation: Option<f64>,
    pub time: Option<chrono::DateTime<chrono::FixedOffset>>,
    pub invalid_time: bool,
    pub start: bool,
    pub distance: f64,
}
#[derive(Clone, Debug, PartialEq)]
pub struct Route {
    pub format: &'static str,
    pub points: Vec<Point>,
    pub distance: f64,
    pub segments: usize,
}
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct Timing {
    pub valid_points: usize,
    pub invalid_points: usize,
    pub missing_points: usize,
    pub eligible_legs: usize,
    pub timed_legs: usize,
    pub untimed_legs: usize,
    pub non_increasing_legs: usize,
    pub seconds: f64,
    pub distance: f64,
    pub average_kmh: Option<f64>,
}
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct ElevationStats {
    pub known_points: usize,
    pub eligible_legs: usize,
    pub covered_legs: usize,
    pub ascent: Option<f64>,
    pub descent: Option<f64>,
}
impl Route {
    pub fn elevation_stats(&self) -> ElevationStats {
        let mut stats = ElevationStats {
            known_points: self.points.iter().filter(|p| p.elevation.is_some()).count(),
            eligible_legs: 0,
            covered_legs: 0,
            ascent: None,
            descent: None,
        };
        let (mut ascent, mut descent) = (0., 0.);
        for pair in self.points.windows(2).filter(|pair| !pair[1].start) {
            stats.eligible_legs += 1;
            if let (Some(a), Some(b)) = (pair[0].elevation, pair[1].elevation) {
                stats.covered_legs += 1;
                let difference = b - a;
                if difference >= 0. {
                    ascent += difference;
                } else {
                    descent -= difference;
                }
            }
        }
        if stats.covered_legs > 0 {
            stats.ascent = Some(ascent);
            stats.descent = Some(descent);
        }
        stats
    }
    pub fn timing(&self) -> Timing {
        let mut stats = Timing {
            valid_points: 0,
            invalid_points: 0,
            missing_points: 0,
            eligible_legs: 0,
            timed_legs: 0,
            untimed_legs: 0,
            non_increasing_legs: 0,
            seconds: 0.,
            distance: 0.,
            average_kmh: None,
        };
        for p in &self.points {
            if p.time.is_some() {
                stats.valid_points += 1;
            } else if p.invalid_time {
                stats.invalid_points += 1;
            } else {
                stats.missing_points += 1;
            }
        }
        for pair in self.points.windows(2).filter(|pair| !pair[1].start) {
            stats.eligible_legs += 1;
            let (Some(a), Some(b)) = (pair[0].time, pair[1].time) else {
                stats.untimed_legs += 1;
                continue;
            };
            let Ok(elapsed) = (b - a).to_std() else {
                stats.non_increasing_legs += 1;
                continue;
            };
            if elapsed.is_zero() {
                stats.non_increasing_legs += 1;
                continue;
            }
            stats.timed_legs += 1;
            stats.seconds += elapsed.as_secs_f64();
            stats.distance += distance(&pair[0], &pair[1]);
        }
        if stats.timed_legs > 0 {
            stats.average_kmh = Some(stats.distance / stats.seconds * 3.6);
        }
        stats
    }
}
pub fn supported(path: &std::path::Path) -> bool {
    path.extension()
        .is_some_and(|s| s.eq_ignore_ascii_case("gpx") || s.eq_ignore_ascii_case("kml"))
}
fn distance(a: &Point, b: &Point) -> f64 {
    let lat = (b.lat - a.lat).to_radians();
    let lon = (b.lon - a.lon).to_radians();
    let h = (lat / 2.).sin().powi(2)
        + a.lat.to_radians().cos() * b.lat.to_radians().cos() * (lon / 2.).sin().powi(2);
    6_371_008.8 * 2. * h.clamp(0., 1.).sqrt().asin()
}
fn timestamp(node: roxmltree::Node<'_, '_>) -> Option<chrono::DateTime<chrono::FixedOffset>> {
    if node.children().any(|n| n.is_element()) {
        return None;
    }
    let text: String = node
        .children()
        .filter(|n| n.is_text())
        .filter_map(|n| n.text())
        .collect();
    chrono::DateTime::parse_from_rfc3339(text.trim()).ok()
}
pub fn parse(text: &str, cancelled: &dyn Fn() -> bool) -> Result<Route, String> {
    if text.len() > LIMIT {
        return Err("Route preview is limited to 256 KiB".into());
    }
    if cancelled() {
        return Err("Cancelled".into());
    }
    let doc = roxmltree::Document::parse_with_options(
        text,
        roxmltree::ParsingOptions {
            allow_dtd: false,
            nodes_limit: 50_000,
        },
    )
    .map_err(|e| format!("Invalid route XML: {e}"))?;
    let root = doc.root_element();
    if root.tag_name().name() == "kml" {
        return parse_kml(root, cancelled);
    }
    let ns = root.tag_name().namespace();
    if root.tag_name().name() != "gpx"
        || !matches!(
            ns,
            None | Some("http://www.topografix.com/GPX/1/0")
                | Some("http://www.topografix.com/GPX/1/1")
        )
    {
        return Err("Not a supported GPX/KML document".into());
    }
    let named = |n: roxmltree::Node<'_, '_>, name: &str| {
        n.is_element() && n.tag_name().name() == name && n.tag_name().namespace() == ns
    };
    let mut route = Route {
        format: "GPX",
        points: Vec::new(),
        distance: 0.,
        segments: 0,
    };
    for container in root
        .descendants()
        .filter(|n| named(*n, "trkseg") || named(*n, "rte"))
    {
        // Only standard top-level routes and track segments, never extension payloads.
        let track = named(container, "trkseg")
            && container
                .parent()
                .is_some_and(|p| named(p, "trk") && p.parent() == Some(root));
        if !track && !(named(container, "rte") && container.parent() == Some(root)) {
            continue;
        }
        let mut first = true;
        for node in container
            .children()
            .filter(|n| named(*n, if track { "trkpt" } else { "rtept" }))
        {
            if cancelled() {
                return Err("Cancelled".into());
            }
            if route.points.len() == POINT_LIMIT {
                return Err("GPX preview is limited to 10,000 points".into());
            }
            let number = |name| {
                node.attribute(name)
                    .and_then(|s| s.trim().parse::<f64>().ok())
                    .filter(|v| v.is_finite())
            };
            let lat = number("lat")
                .filter(|v| (-90. ..=90.).contains(v))
                .ok_or("Invalid GPX latitude")?;
            let lon = number("lon")
                .filter(|v| (-180. ..=180.).contains(v))
                .ok_or("Invalid GPX longitude")?;
            let elevation = match node.children().find(|n| named(*n, "ele")) {
                None => None,
                Some(n) => Some(
                    n.text()
                        .and_then(|s| s.trim().parse::<f64>().ok())
                        .filter(|v| v.is_finite() && (-20_000. ..=100_000.).contains(v))
                        .ok_or("Invalid GPX elevation")?,
                ),
            };
            let times: Vec<_> = node.children().filter(|n| named(*n, "time")).collect();
            let time = if times.len() == 1 {
                timestamp(times[0])
            } else {
                None
            };
            let invalid_time = !times.is_empty() && time.is_none();
            let mut point = Point {
                lat,
                lon,
                elevation,
                time,
                invalid_time,
                start: first,
                distance: route.distance,
            };
            if !first {
                route.distance += distance(route.points.last().unwrap(), &point);
            } else {
                route.segments += 1;
            }
            point.distance = route.distance;
            route.points.push(point);
            first = false;
        }
    }
    if route.points.is_empty() {
        return Err(
            "GPX contains no track or route points (waypoints alone are not a route)".into(),
        );
    }
    Ok(route)
}
const KML: &str = "http://www.opengis.net/kml/2.2";
const GX: &str = "http://www.google.com/kml/ext/2.2";
fn is(node: roxmltree::Node<'_, '_>, ns: Option<&str>, name: &str) -> bool {
    node.is_element() && node.tag_name().namespace() == ns && node.tag_name().name() == name
}
fn parse_kml(root: roxmltree::Node<'_, '_>, cancelled: &dyn Fn() -> bool) -> Result<Route, String> {
    let ns = root.tag_name().namespace();
    if !matches!(ns, None | Some(KML)) {
        return Err("Unsupported KML namespace".into());
    }
    let mut route = Route {
        format: "KML",
        points: Vec::new(),
        distance: 0.,
        segments: 0,
    };
    for geometry in root
        .descendants()
        .filter(|n| is(*n, ns, "LineString") || is(*n, Some(GX), "Track"))
    {
        if cancelled() {
            return Err("Cancelled".into());
        }
        // Geometry must belong to a standard Placemark, never ExtendedData,
        // descriptions, styles, polygons, foreign namespaces or linked files.
        let mut parent = geometry.parent();
        let mut placemark = false;
        let mut valid = false;
        while let Some(n) = parent {
            if n == root {
                valid = placemark;
                break;
            }
            if !placemark {
                if is(n, ns, "Placemark") {
                    placemark = true;
                } else if !is(n, ns, "MultiGeometry") && !is(n, Some(GX), "MultiTrack") {
                    break;
                }
            } else if !is(n, ns, "Document") && !is(n, ns, "Folder") {
                break;
            }
            parent = n.parent();
        }
        if !valid {
            continue;
        }
        let modes: Vec<_> = geometry
            .children()
            .filter(|n| is(*n, ns, "altitudeMode") || is(*n, Some(GX), "altitudeMode"))
            .collect();
        if modes.len() > 1 {
            return Err("Ambiguous KML altitude mode".into());
        }
        let absolute = if let Some(mode) = modes.first() {
            match (
                mode.tag_name().namespace() == Some(GX),
                mode.text().unwrap_or("").trim(),
            ) {
                (false, "absolute") => true,
                (false, "clampToGround" | "relativeToGround")
                | (true, "clampToSeaFloor" | "relativeToSeaFloor") => false,
                _ => return Err("Unsupported KML altitude mode".into()),
            }
        } else {
            false
        };
        let track = is(geometry, Some(GX), "Track");
        let first = std::cell::Cell::new(true);
        let mut add = |parts: Vec<&str>,
                       time: Option<chrono::DateTime<chrono::FixedOffset>>,
                       invalid_time: bool|
         -> Result<(), String> {
            if cancelled() {
                return Err("Cancelled".into());
            }
            if !(2..=3).contains(&parts.len()) {
                return Err(
                    "KML coordinates need longitude, latitude and optional altitude".into(),
                );
            }
            let num = |index: usize| {
                parts[index]
                    .parse::<f64>()
                    .ok()
                    .filter(|v| v.is_finite())
                    .ok_or("Invalid KML coordinate")
            };
            let lon = num(0)?;
            let lat = num(1)?;
            if !(-90. ..=90.).contains(&lat) || !(-180. ..=180.).contains(&lon) {
                return Err("KML coordinate outside latitude/longitude bounds".into());
            }
            let altitude = if parts.len() == 3 {
                Some(num(2)?)
            } else {
                None
            };
            let elevation = if absolute { altitude } else { None };
            if elevation.is_some_and(|e| !(-20_000. ..=100_000.).contains(&e)) {
                return Err("KML absolute altitude outside preview bounds".into());
            }
            if route.points.len() == POINT_LIMIT {
                return Err("Route preview is limited to 10,000 points".into());
            }
            let mut point = Point {
                lat,
                lon,
                elevation,
                time,
                invalid_time,
                start: first.get(),
                distance: route.distance,
            };
            if first.get() {
                route.segments += 1;
            } else {
                route.distance += distance(route.points.last().unwrap(), &point);
            }
            point.distance = route.distance;
            route.points.push(point);
            first.set(false);
            Ok(())
        };
        if track {
            // Sparse gx positions stay gaps; no invented interpolation.
            let coords: Vec<_> = geometry
                .children()
                .filter(|n| is(*n, Some(GX), "coord"))
                .collect();
            let times: Vec<_> = geometry.children().filter(|n| is(*n, ns, "when")).collect();
            let aligned = coords.len() == times.len();
            for (index, coord) in coords.into_iter().enumerate() {
                if coord.children().any(|n| n.is_element()) {
                    return Err("Invalid nested gx:coord".into());
                }
                let text: String = coord
                    .children()
                    .filter_map(|n| n.text().filter(|_| n.is_text()))
                    .collect();
                let text = text.trim();
                if text.is_empty() {
                    first.set(true);
                    continue;
                }
                // Longitude/latitude tuple order differs from GPX attributes.
                let time = if aligned {
                    timestamp(times[index])
                } else {
                    None
                };
                let invalid_time = !times.is_empty() && time.is_none();
                add(text.split_whitespace().collect(), time, invalid_time)?;
            }
        } else {
            let coords: Vec<_> = geometry
                .children()
                .filter(|n| is(*n, ns, "coordinates"))
                .collect();
            if coords.len() != 1 || coords[0].children().any(|n| n.is_element()) {
                return Err("KML LineString needs one coordinates element".into());
            }
            let text: String = coords[0]
                .children()
                .filter_map(|n| n.text().filter(|_| n.is_text()))
                .collect();
            for tuple in text.split_whitespace() {
                add(tuple.split(',').collect(), None, false)?;
            }
        }
    }
    if route.points.is_empty() {
        return Err("KML contains no supported LineString or gx:Track coordinates".into());
    }
    Ok(route)
}
// Preserve segment boundaries and unwrap longitude locally across the date line.
fn projected(route: &Route) -> Vec<[f64; 2]> {
    let reference = route.points[0].lon;
    let mean = route.points.iter().map(|p| p.lat).sum::<f64>() / route.points.len() as f64;
    let scale = mean.to_radians().cos().abs().max(0.000001);
    let mut previous = reference;
    route
        .points
        .iter()
        .map(|p| {
            let anchor = if p.start { reference } else { previous };
            let lon = anchor + (p.lon - anchor + 180.).rem_euclid(360.) - 180.;
            previous = lon;
            [lon * scale, -p.lat]
        })
        .collect()
}
fn nearest_point(points: &[Pos2], cursor: Pos2) -> Option<usize> {
    points
        .iter()
        .enumerate()
        .map(|(index, point)| (index, point.distance_sq(cursor)))
        .filter(|(_, distance)| *distance <= 12. * 12.)
        .min_by(|(ai, ad), (bi, bd)| ad.total_cmp(bd).then(ai.cmp(bi)))
        .map(|(index, _)| index)
}
fn point_details(route: &Route, index: usize) -> Vec<String> {
    let point = &route.points[index];
    let segment = route.points[..=index].iter().filter(|p| p.start).count();
    vec![
        format!(
            "Point {} / {} · segment {segment}",
            index + 1,
            route.points.len()
        ),
        format!("Latitude: {:.6}°", point.lat),
        format!("Longitude: {:.6}°", point.lon),
        format!("Recorded distance: {:.1} m", point.distance),
        point.elevation.map_or_else(
            || "Elevation: unavailable".into(),
            |e| format!("Elevation: {e:.1} m"),
        ),
        point.time.map_or_else(
            || {
                if point.invalid_time {
                    "Time: invalid".into()
                } else {
                    "Time: not recorded".into()
                }
            },
            |time| format!("Time: {}", time.to_rfc3339()),
        ),
    ]
}
fn positions(values: &[[f64; 2]], rect: Rect) -> Vec<Pos2> {
    let mut lo = [f64::INFINITY; 2];
    let mut hi = [f64::NEG_INFINITY; 2];
    for p in values {
        for i in 0..2 {
            lo[i] = lo[i].min(p[i]);
            hi[i] = hi[i].max(p[i]);
        }
    }
    let scale = (f64::from(rect.width()) / (hi[0] - lo[0]).max(1e-9))
        .min(f64::from(rect.height()) / (hi[1] - lo[1]).max(1e-9));
    values
        .iter()
        .map(|p| {
            rect.center()
                + Vec2::new(
                    ((p[0] - (lo[0] + hi[0]) / 2.) * scale) as f32,
                    ((p[1] - (lo[1] + hi[1]) / 2.) * scale) as f32,
                )
        })
        .collect()
}
#[derive(Clone)]
struct Cached(Arc<Job>);
struct Job {
    text: Arc<str>,
    result: Mutex<Option<Result<Arc<Route>, String>>>,
    cancel: Arc<AtomicBool>,
}
impl Drop for Job {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}
static ACTIVE: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
fn load(ctx: &egui::Context, text: &Arc<str>, side: &str) -> Option<Result<Arc<Route>, String>> {
    let id = egui::Id::new(("gpx-route-cache", side));
    let cached = ctx.data(|d| d.get_temp::<Cached>(id));
    if let Some(cached) = cached.filter(|c| Arc::ptr_eq(&c.0.text, text)) {
        return cached.0.result.lock().ok()?.clone();
    }
    if ACTIVE
        .try_update(Ordering::AcqRel, Ordering::Relaxed, |n| {
            (n < 2).then_some(n + 1)
        })
        .is_err()
    {
        ctx.request_repaint_after(std::time::Duration::from_millis(30));
        return None;
    }
    let cancel = Arc::new(AtomicBool::new(false));
    let job = Arc::new(Job {
        text: text.clone(),
        result: Mutex::new(None),
        cancel: cancel.clone(),
    });
    let weak = Arc::downgrade(&job);
    let input = text.clone();
    let repaint = ctx.clone();
    let spawned = std::thread::Builder::new()
        .name("gpx-preview".into())
        .spawn(move || {
            struct Permit;
            impl Drop for Permit {
                fn drop(&mut self) {
                    ACTIVE.fetch_sub(1, Ordering::AcqRel);
                }
            }
            let _permit = Permit;
            let result = parse(&input, &|| cancel.load(Ordering::Relaxed)).map(Arc::new);
            if let Some(job) = weak.upgrade() {
                *job.result.lock().unwrap() = Some(result);
                repaint.request_repaint();
            }
        });
    if let Err(error) = spawned {
        ACTIVE.fetch_sub(1, Ordering::AcqRel);
        *job.result.lock().unwrap() = Some(Err(format!("Cannot start GPX preview: {error}")));
    }
    ctx.data_mut(|d| d.insert_temp(id, Cached(job)));
    None
}
pub fn render(ui: &mut egui::Ui, text: &Arc<str>, side: &str) -> Option<String> {
    ui.small("Offline schematic · distances are spherical estimates · no map tiles");
    let Some(result) = load(ui.ctx(), text, side) else {
        ui.spinner();
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(30));
        return None;
    };
    let route = match result {
        Ok(route) => route,
        Err(e) => {
            ui.colored_label(ui.visuals().error_fg_color, e);
            return None;
        }
    };
    let mut copied = None;
    let error_id = egui::Id::new(("route-report-error", side));
    ui.horizontal_wrapped(|ui| {
        ui.label(format!("{} · {} points · {} segments · {:.2} km",route.format,route.points.len(),route.segments,route.distance/1000.));
        let response=ui.small_button("Copy route JSON").on_hover_text("Copy the parsed points, segments and recorded statistics as Commander JSON (up to 1 MiB). No new file reads.");
        #[cfg(feature="visual-qa")]
        ui.ctx().data_mut(|data|data.insert_temp(egui::Id::new(("route-report-copy",side)),response.rect));
        if response.clicked() {
            match crate::route_report::json(&route) {
                Ok(text)=> {copied=Some(text);ui.ctx().data_mut(|data|data.remove::<(Arc<Route>,String)>(error_id));},
                Err(error)=>{ui.ctx().data_mut(|data|data.insert_temp(error_id,(route.clone(),error)));},
            }
        }
    });
    if let Some((owner, error)) = ui
        .ctx()
        .data(|data| data.get_temp::<(Arc<Route>, String)>(error_id))
    {
        if Arc::ptr_eq(&owner, &route) {
            ui.colored_label(ui.visuals().error_fg_color, error);
        } else {
            ui.ctx()
                .data_mut(|data| data.remove::<(Arc<Route>, String)>(error_id));
        }
    }

    if route.format == "GPX"
        || route
            .points
            .iter()
            .any(|p| p.time.is_some() || p.invalid_time)
    {
        let timing = route.timing();
        if let Some(speed) = timing.average_kmh {
            ui.label(format!(
                "Timed legs: {}/{} · {:.3} s · {:.2} km covered · {:.2} km/h average",
                timing.timed_legs,
                timing.eligible_legs,
                timing.seconds,
                timing.distance / 1000.,
                speed
            ));
        } else {
            ui.small("No increasing timestamps on adjacent route points; timing unavailable.");
        }
        ui.small(format!(
            "Timestamps: {} valid · {} missing · {} invalid · {} non-increasing legs",
            timing.valid_points,
            timing.missing_points,
            timing.invalid_points,
            timing.non_increasing_legs
        ));
        ui.small("Timing covers adjacent points within each segment; gaps are excluded. Average includes recorded stops, not moving speed.");
    }
    if route.format == "KML" {
        ui.small(
            "Absolute altitude only · missing gx positions stay gaps · styles and links ignored",
        );
    }
    let size = Vec2::new(
        ui.available_width().max(1.),
        (ui.available_height() - 160.).clamp(80., 450.),
    );
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::hover());
    let stroke = Stroke::new(2., ui.visuals().selection.stroke.color);
    let points = positions(&projected(&route), rect.shrink(12.));
    ui.painter()
        .rect_filled(rect, 4., ui.visuals().extreme_bg_color);
    for i in 0..points.len() {
        if i > 0 && !route.points[i].start {
            ui.painter()
                .line_segment([points[i - 1], points[i]], stroke);
        }
        if route.points[i].start {
            ui.painter()
                .circle_filled(points[i], 4., Color32::LIGHT_GREEN);
        }
    }
    if let Some(last) = points.last() {
        ui.painter().circle_filled(*last, 4., Color32::LIGHT_RED);
    }
    #[cfg(feature = "visual-qa")]
    ui.ctx().data_mut(|data| {
        data.insert_temp(
            egui::Id::new(("route-point-positions", side)),
            points.clone(),
        );
        data.remove::<(usize, Vec<String>)>(egui::Id::new(("route-point-tooltip", side)));
    });
    if let Some(index) = response
        .hover_pos()
        .and_then(|cursor| nearest_point(&points, cursor))
    {
        ui.painter().circle_stroke(
            points[index],
            6.,
            Stroke::new(2., ui.visuals().text_color()),
        );
        let details = point_details(&route, index);
        response.on_hover_ui(|ui| {
            for line in &details {
                ui.label(line);
            }
            #[cfg(feature = "visual-qa")]
            ui.ctx().data_mut(|data| {
                data.insert_temp(
                    egui::Id::new(("route-point-tooltip", side)),
                    (index, details.clone()),
                )
            });
        });
    }
    let elevations: Vec<_> = route.points.iter().filter_map(|p| p.elevation).collect();
    if !elevations.is_empty() {
        let low = elevations.iter().copied().fold(f64::INFINITY, f64::min);
        let high = elevations.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        ui.label(format!(
            "Recorded elevation: {low:.0}–{high:.0} m · {} / {} points",
            elevations.len(),
            route.points.len()
        ));
        let coverage = route.elevation_stats();
        if let (Some(up), Some(down)) = (coverage.ascent, coverage.descent) {
            ui.small(format!("Recorded gain/loss: +{up:.1} / −{down:.1} m · {} / {} adjacent legs · raw samples, gaps excluded",coverage.covered_legs,coverage.eligible_legs));
        } else {
            ui.small("Gain/loss unavailable: no adjacent elevations within a segment.");
        }
        let (rect, _) = ui.allocate_exact_size(Vec2::new(size.x, 80.), egui::Sense::hover());
        let r = rect.shrink(6.);
        ui.painter()
            .rect_filled(rect, 4., ui.visuals().extreme_bg_color);
        let at = |p: &Point| {
            p.elevation.map(|e| {
                Pos2::new(
                    r.left()
                        + ((p.distance / route.distance.max(1.)) * f64::from(r.width())) as f32,
                    r.bottom()
                        - (((e - low) / (high - low).max(1.)) * f64::from(r.height())) as f32,
                )
            })
        };
        for pair in route.points.windows(2) {
            if !pair[1].start
                && let (Some(a), Some(b)) = (at(&pair[0]), at(&pair[1]))
            {
                ui.painter().line_segment([a, b], stroke);
            }
        }
    }
    #[cfg(feature = "visual-qa")]
    ui.ctx()
        .data_mut(|d| d.insert_temp(egui::Id::new(("gpx-route-ready", side)), route));
    copied
}
#[cfg(any(test, feature = "visual-qa"))]
pub fn fixture() -> &'static str {
    r#"<?xml version="1.0"?><gpx xmlns="http://www.topografix.com/GPX/1/1"><trk><name>Commander route</name><trkseg><trkpt lat="41.70" lon="44.79"><ele>420</ele><time>2026-11-02T00:00:00Z</time></trkpt><trkpt lat="41.71" lon="44.80"><ele>460</ele><time>2026-11-02T00:01:00Z</time></trkpt><trkpt lat="41.72" lon="44.79"><ele>510</ele><time>2026-11-02T00:03:00Z</time></trkpt></trkseg><trkseg><trkpt lat="41.72" lon="44.81"><ele>490</ele><time>2026-11-02T01:00:00Z</time></trkpt><trkpt lat="41.71" lon="44.82"><ele>450</ele><time>2026-11-02T01:02:00Z</time></trkpt><trkpt lat="41.70" lon="44.81"><ele>430</ele><time>2026-11-02T01:05:00Z</time></trkpt></trkseg></trk></gpx>"#
}
#[cfg(any(test, feature = "visual-qa"))]
pub fn kml_fixture() -> &'static str {
    r#"<?xml version="1.0"?><kml xmlns="http://www.opengis.net/kml/2.2" xmlns:gx="http://www.google.com/kml/ext/2.2"><Document><Placemark><MultiGeometry><LineString><altitudeMode>absolute</altitudeMode><coordinates>44.79,41.70,420 44.80,41.71,460 44.79,41.72,510</coordinates></LineString><gx:Track><altitudeMode>absolute</altitudeMode><when>2026-11-02T01:00:00Z</when><when>2026-11-02T01:02:00Z</when><when>2026-11-02T01:05:00Z</when><gx:coord>44.81 41.72 490</gx:coord><gx:coord>44.82 41.71 450</gx:coord><gx:coord>44.81 41.70 430</gx:coord></gx:Track></MultiGeometry></Placemark></Document></kml>"#
}
#[cfg(test)]
mod tests {
    use super::*;
    pub const FIXTURE: &str = r#"<gpx xmlns="http://www.topografix.com/GPX/1/1"><trk><trkseg><trkpt lat="0" lon="0"><ele>10</ele></trkpt><trkpt lat="0" lon="1"><ele>30</ele></trkpt></trkseg><trkseg><trkpt lat="10" lon="20"/><trkpt lat="10" lon="21"><ele>20</ele></trkpt></trkseg></trk><rte><rtept lat="0" lon="179.9"/><rtept lat="0" lon="-179.9"/></rte></gpx>"#;
    #[test]
    fn point_hit_testing_is_bounded_and_deterministic_without_selecting_route_lines() {
        let points = [Pos2::new(0., 0.), Pos2::new(20., 0.), Pos2::new(20., 0.)];
        assert_eq!(nearest_point(&points, Pos2::new(20., 0.)), Some(1));
        assert_eq!(nearest_point(&points, Pos2::new(10., 0.)), Some(0));
        assert_eq!(nearest_point(&points, Pos2::new(20., 12.)), Some(1));
        assert_eq!(nearest_point(&points, Pos2::new(20., 12.1)), None);
        assert_eq!(nearest_point(&[], Pos2::ZERO), None);
    }
    #[test]
    fn point_details_keep_segment_owner_timestamp_and_missing_states() {
        let mut route = parse(fixture(), &|| false).unwrap();
        let details = point_details(&route, 3);
        assert_eq!(details[0], "Point 4 / 6 · segment 2");
        assert_eq!(details[1], "Latitude: 41.720000°");
        assert_eq!(details[2], "Longitude: 44.810000°");
        assert_eq!(details[4], "Elevation: 490.0 m");
        assert_eq!(details[5], "Time: 2026-11-02T01:00:00+00:00");
        route.points[3].elevation = None;
        route.points[3].time = None;
        let details = point_details(&route, 3);
        assert_eq!(details[4], "Elevation: unavailable");
        assert_eq!(details[5], "Time: not recorded");
        route.points[3].invalid_time = true;
        assert_eq!(point_details(&route, 3)[5], "Time: invalid");
    }
    #[test]
    fn elevation_gain_loss_counts_adjacent_samples_without_segment_or_missing_value_bridges() {
        let route = parse(fixture(), &|| false).unwrap();
        assert_eq!(
            route.elevation_stats(),
            ElevationStats {
                known_points: 6,
                eligible_legs: 4,
                covered_legs: 4,
                ascent: Some(90.),
                descent: Some(60.)
            }
        );
        assert_eq!(
            parse(kml_fixture(), &|| false).unwrap().elevation_stats(),
            route.elevation_stats()
        );
        let missing = fixture().replace("<ele>460</ele>", "");
        let stats = parse(&missing, &|| false).unwrap().elevation_stats();
        assert_eq!(stats.known_points, 5);
        assert_eq!(stats.covered_legs, 2);
        assert_eq!(stats.ascent, Some(0.));
        assert_eq!(stats.descent, Some(60.));
    }
    #[test]
    fn absent_or_isolated_elevations_are_not_reported_as_zero_gain_and_relative_altitude_is_not_terrain()
     {
        let isolated = r#"<gpx><trk><trkseg><trkpt lat="0" lon="0"><ele>1000</ele></trkpt></trkseg><trkseg><trkpt lat="0" lon="0"><ele>-1000</ele></trkpt></trkseg></trk></gpx>"#;
        let stats = parse(isolated, &|| false).unwrap().elevation_stats();
        assert_eq!(
            (stats.known_points, stats.eligible_legs, stats.covered_legs),
            (2, 0, 0)
        );
        assert_eq!((stats.ascent, stats.descent), (None, None));
        let relative = kml_fixture().replace(
            "<altitudeMode>absolute</altitudeMode>",
            "<altitudeMode>relativeToGround</altitudeMode>",
        );
        let stats = parse(&relative, &|| false).unwrap().elevation_stats();
        assert_eq!(
            (stats.known_points, stats.covered_legs, stats.ascent),
            (0, 0, None)
        );
        let flat = fixture()
            .replace("<ele>460</ele>", "<ele>420</ele>")
            .replace("<ele>510</ele>", "<ele>420</ele>")
            .replace("<ele>450</ele>", "<ele>490</ele>")
            .replace("<ele>430</ele>", "<ele>490</ele>");
        let stats = parse(&flat, &|| false).unwrap().elevation_stats();
        assert_eq!((stats.ascent, stats.descent), (Some(0.), Some(0.)));
    }
    #[test]
    fn kml_track_time_alignment_preserves_sparse_indices_and_refuses_mismatched_arrays() {
        let text = r#"<kml xmlns="http://www.opengis.net/kml/2.2" xmlns:gx="http://www.google.com/kml/ext/2.2"><Placemark><gx:Track><when>2026-01-01T00:00:00Z</when><when>2026-01-01T00:01:00Z</when><when>2026-01-01T00:02:00Z</when><when>2026-01-01T00:03:00Z</when><gx:coord>0 0 0</gx:coord><gx:coord/><gx:coord>2 0 0</gx:coord><gx:coord>3 0 0</gx:coord></gx:Track></Placemark></kml>"#;
        let route = parse(text, &|| false).unwrap();
        let t = route.timing();
        assert_eq!(route.points.len(), 3);
        assert!(route.points[1].start);
        assert_eq!(
            route.points[1].time.unwrap().to_rfc3339(),
            "2026-01-01T00:02:00+00:00"
        );
        assert_eq!(
            (t.valid_points, t.eligible_legs, t.timed_legs, t.seconds),
            (3, 1, 1, 60.)
        );
        let bad = text.replace("<when>2026-01-01T00:01:00Z</when>", "");
        let r = parse(&bad, &|| false).unwrap();
        let t = r.timing();
        assert_eq!(r.distance, route.distance);
        assert_eq!((t.invalid_points, t.valid_points, t.timed_legs), (3, 0, 0));
        assert_eq!(t.average_kmh, None);
        let malformed = text.replace("2026-01-01T00:03:00Z", "bad");
        let t = parse(&malformed, &|| false).unwrap().timing();
        assert_eq!((t.invalid_points, t.valid_points, t.timed_legs), (1, 2, 0));
    }
    #[test]
    fn mixed_kml_geometry_times_only_track_legs_and_ignores_foreign_when() {
        let route = parse(kml_fixture(), &|| false).unwrap();
        let t = route.timing();
        assert_eq!(
            (
                t.valid_points,
                t.missing_points,
                t.eligible_legs,
                t.timed_legs,
                t.untimed_legs
            ),
            (3, 3, 4, 2, 2)
        );
        assert_eq!(t.seconds, 300.);
        assert!(t.distance < route.distance);
        let foreign = kml_fixture()
            .replace("<when>", r#"<x:when xmlns:x="urn:foreign">"#)
            .replace("</when>", "</x:when>");
        let t = parse(&foreign, &|| false).unwrap().timing();
        assert_eq!(
            (t.valid_points, t.invalid_points, t.missing_points),
            (0, 0, 6)
        );
    }
    #[test]
    fn timing_uses_only_adjacent_increasing_points_and_never_crosses_segment_gaps() {
        let route = parse(fixture(), &|| false).unwrap();
        let t = route.timing();
        assert_eq!((t.valid_points, t.eligible_legs, t.timed_legs), (6, 4, 4));
        assert_eq!(t.seconds, 480.);
        assert!((t.distance - route.distance).abs() < 1e-6);
        assert!((t.average_kmh.unwrap() - route.distance / 480. * 3.6).abs() < 1e-9);
        let missing = fixture().replace("<time>2026-11-02T00:01:00Z</time>", "");
        let t = parse(&missing, &|| false).unwrap().timing();
        assert_eq!(
            (
                t.valid_points,
                t.missing_points,
                t.timed_legs,
                t.untimed_legs
            ),
            (5, 1, 2, 2)
        );
        assert_eq!(t.seconds, 300.);
    }
    #[test]
    fn timing_handles_offsets_subseconds_invalid_duplicates_and_non_increasing_times() {
        let text = r#"<gpx><trk><trkseg><trkpt lat="0" lon="0"><time>1969-12-31T23:59:59.5Z</time></trkpt><trkpt lat="0" lon="0"><time>1970-01-01T01:00:00+01:00</time></trkpt><trkpt lat="0" lon="1"><time>1970-01-01T00:00:00Z</time></trkpt><trkpt lat="0" lon="2"><time>1969-12-31T23:59:59Z</time></trkpt><trkpt lat="0" lon="3"><time>bad</time></trkpt><trkpt lat="0" lon="4"><time>1970-01-01T00:00:00Z</time><time>1970-01-01T00:00:01Z</time></trkpt><trkpt lat="0" lon="5"><time><x>1970-01-01T00:00:02Z</x></time></trkpt><trkpt lat="0" lon="6"/></trkseg></trk></gpx>"#;
        let r = parse(text, &|| false).unwrap();
        let t = r.timing();
        assert_eq!(
            (t.valid_points, t.invalid_points, t.missing_points),
            (4, 3, 1)
        );
        assert_eq!(
            (t.timed_legs, t.non_increasing_legs, t.untimed_legs),
            (1, 2, 4)
        );
        assert_eq!(t.seconds, 0.5);
        assert_eq!(t.average_kmh, Some(0.));
        assert_eq!(
            parse(FIXTURE, &|| false).unwrap().timing().average_kmh,
            None
        );
    }
    #[test]
    fn kml_lines_tracks_gaps_and_altitude_modes() {
        let text = r#"<kml xmlns="http://www.opengis.net/kml/2.2" xmlns:gx="http://www.google.com/kml/ext/2.2"><Document><Folder><Placemark><MultiGeometry><LineString><altitudeMode>absolute</altitudeMode><coordinates>0,0,10 <!-- split text --> 1,0,20</coordinates></LineString><LineString><coordinates>20,10,999 21,10,999</coordinates></LineString><gx:MultiTrack><gx:Track><altitudeMode>absolute</altitudeMode><gx:coord>179.9 0 30</gx:coord><gx:coord>-179.9 0 40</gx:coord><gx:coord/><gx:coord>0 50 50</gx:coord></gx:Track></gx:MultiTrack></MultiGeometry></Placemark></Folder></Document></kml>"#;
        assert_eq!(parse(kml_fixture(), &|| false).unwrap().points.len(), 6);
        let r = parse(text, &|| false).unwrap();
        assert_eq!(r.format, "KML");
        assert_eq!(r.points.len(), 7);
        assert_eq!(r.segments, 4);
        assert_eq!(r.points[2].elevation, None);
        assert_eq!(r.points[4].elevation, Some(30.));
        assert_eq!(r.points[6].distance, r.points[5].distance);
        assert!((r.distance - 242939.83147948177).abs() < 0.01);
        assert_eq!(r.points[1].lon, 1.);
        assert_eq!(r.points[1].lat, 0.);
        for mode in ["relativeToGround", "clampToGround"] {
            let s = format!(
                "<kml><Placemark><LineString><altitudeMode>{mode}</altitudeMode><coordinates>1,2,100</coordinates></LineString></Placemark></kml>"
            );
            assert_eq!(parse(&s, &|| false).unwrap().points[0].elevation, None);
        }
    }
    #[test]
    fn kml_refuses_bad_coordinates_modes_and_embedded_geometry() {
        for value in [
            "NaN,0",
            "0,91",
            "181,0",
            "0,0,NaN",
            "0,0,1,2",
            ",0",
            "0,0,100001",
        ] {
            let s = format!(
                "<kml><Placemark><LineString><altitudeMode>absolute</altitudeMode><coordinates>{value}</coordinates></LineString></Placemark></kml>"
            );
            assert!(parse(&s, &|| false).is_err(), "{value}");
        }
        for text in [
            "<kml xmlns='foreign'><Placemark><LineString><coordinates>0,0</coordinates></LineString></Placemark></kml>",
            "<kml><ExtendedData><Placemark><LineString><coordinates>0,0</coordinates></LineString></Placemark></ExtendedData></kml>",
            "<kml><Placemark><Point><coordinates>0,0</coordinates></Point></Placemark></kml>",
            "<kml><Placemark><LineString><altitudeMode>absolute</altitudeMode><altitudeMode>relativeToGround</altitudeMode><coordinates>0,0,1</coordinates></LineString></Placemark></kml>",
            "<kml><Placemark><LineString><coordinates>0,0</coordinates><coordinates>1,1</coordinates></LineString></Placemark></kml>",
            "<kml><Placemark><LineString><coordinates><fake>0,0</fake></coordinates></LineString></Placemark></kml>",
        ] {
            assert!(parse(text, &|| false).is_err(), "{text}");
        }
        let s = format!(
            "<kml><Placemark><LineString><coordinates>{}</coordinates></LineString></Placemark></kml>",
            "0,0 ".repeat(POINT_LIMIT + 1)
        );
        assert!(parse(&s, &|| false).is_err());
        assert!(parse(&s, &|| true).is_err());
    }
    #[test]
    fn asynchronous_cache_replaces_stale_content_and_isolates_panes() {
        let ctx = egui::Context::default();
        let good: Arc<str> = Arc::from(fixture());
        let invalid: Arc<str> = Arc::from("<gpx/>");
        let wait = |text: &Arc<str>, side: &str| {
            let until = std::time::Instant::now() + std::time::Duration::from_secs(2);
            loop {
                if let Some(r) = load(&ctx, text, side) {
                    break r;
                }
                assert!(std::time::Instant::now() < until);
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
        };
        let first = wait(&good, "left").unwrap();
        assert_eq!(first.points.len(), 6);
        assert_eq!(first.segments, 2);
        assert!(Arc::ptr_eq(&first, &wait(&good, "left").unwrap()));
        assert!(wait(&invalid, "right").is_err());
        assert!(Arc::ptr_eq(&first, &wait(&good, "left").unwrap()));
        // A replacement never returns the previous route while its worker is pending.
        assert!(load(&ctx, &invalid, "left").is_none());
        assert!(wait(&invalid, "left").is_err());
        assert!(supported(std::path::Path::new("route.GPX")));
        assert!(!supported(std::path::Path::new("route.fit")));
    }
    #[test]
    fn segments_distance_elevation_and_date_line() {
        let r = parse(FIXTURE, &|| false).unwrap();
        assert_eq!(r.points.len(), 6);
        assert_eq!(r.segments, 3);
        let expected = 111_195.0802335329 + 109_505.73519924355 + 22_239.016046705317;
        assert!((r.distance - expected).abs() < 0.01);
        assert_eq!(r.points[2].distance, r.points[1].distance);
        assert_eq!(r.points[0].elevation, Some(10.));
        let p = projected(&r);
        assert!((p[5][0] - p[4][0]).abs() < 0.21);
        assert_eq!(r.points.iter().filter(|p| p.start).count(), 3);
        let p = positions(&p, Rect::from_min_size(Pos2::ZERO, Vec2::new(300., 200.)));
        assert!(p.iter().all(|p| p.x.is_finite()
            && p.y.is_finite()
            && p.x >= 0.
            && p.x <= 300.
            && p.y >= 0.
            && p.y <= 200.));
    }
    #[test]
    fn rejects_invalid_incomplete_foreign_and_hostile_documents() {
        for text in [
            "<gpx>",
            "<gpx/>",
            "<gpx><wpt lat='0' lon='0'/></gpx>",
            "<gpx xmlns='evil'><rte><rtept lat='0' lon='0'/></rte></gpx>",
            "<gpx><rte><rtept lat='NaN' lon='0'/></rte></gpx>",
            "<gpx><rte><rtept lat='91' lon='0'/></rte></gpx>",
            "<gpx><rte><rtept lat='0' lon='0'><ele>NaN</ele></rtept></rte></gpx>",
            "<!DOCTYPE gpx [<!ENTITY e 'x'>]><gpx/>",
        ] {
            assert!(parse(text, &|| false).is_err(), "{text}");
        }
        assert!(parse(FIXTURE, &|| true).is_err());
        assert!(parse(&"x".repeat(LIMIT + 1), &|| false).is_err());
        let huge = format!(
            "<gpx><rte>{}</rte></gpx>",
            "<rtept lat='0' lon='0'/>".repeat(POINT_LIMIT + 1)
        );
        assert!(parse(&huge, &|| false).is_err());
    }
    #[test]
    fn namespace_single_point_and_extension_payloads() {
        let r=parse("<g:gpx xmlns:g='http://www.topografix.com/GPX/1/0'><g:rte><g:rtept lat='90' lon='-180'/></g:rte><g:extensions><g:rte><g:rtept lat='0' lon='0'/></g:rte></g:extensions></g:gpx>",&||false).unwrap();
        assert_eq!(r.points.len(), 1);
        assert_eq!(r.distance, 0.);
        assert_eq!(
            positions(
                &projected(&r),
                Rect::from_min_size(Pos2::ZERO, Vec2::splat(100.))
            )[0],
            Pos2::new(50., 50.)
        );
    }
}
