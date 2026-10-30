use crate::geometry::{Rect, Vec2};
use crate::spring::Spring;

/// How far (as a fraction of the view) the cursor may stray from the zoom
/// anchor before the camera stops anchoring and follows it instead.
const ANCHOR_RELEASE: f64 = 0.05;

/// The cursor is never allowed closer than this (as a fraction of the view)
/// to the visible edge, however fast it moves.
const EDGE_MARGIN: f64 = 0.04;

#[derive(Clone, Debug)]
pub struct CameraConfig {
    pub levels: Vec<f64>,
    pub pan_omega: f64,
    pub dead_zone: f64,
    pub zoom_omega: f64,
    pub output_aspect: f64,
}

impl Default for CameraConfig {
    fn default() -> Self {
        Self {
            levels: vec![1.0, 1.5, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0],
            pan_omega: 7.0,
            dead_zone: 0.7,
            zoom_omega: 13.0,
            output_aspect: 16.0 / 9.0,
        }
    }
}

/// Keeps one source point at a fixed spot in the viewport while the zoom
/// animates, so zooming feels like it happens "around" the cursor.
#[derive(Clone, Copy, Debug)]
struct Anchor {
    point: Vec2,
    offset: Vec2,
}

#[derive(Clone, Debug)]
pub struct Camera {
    level: usize,
    source: Vec2,
    is_locked: bool,
    has_source: bool,
    cursor: Option<Vec2>,
    anchor: Option<Anchor>,
    config: CameraConfig,
    center: Spring<Vec2>,
    log_zoom: Spring<f64>,
}

impl Camera {
    pub fn new(config: CameraConfig) -> Self {
        Self {
            level: 1,
            source: Vec2::new(1.0, 1.0),
            is_locked: false,
            has_source: false,
            cursor: None,
            anchor: None,
            center: Spring::new(Vec2::new(0.5, 0.5), config.pan_omega),
            log_zoom: Spring::new(0.0, config.zoom_omega),
            config,
        }
    }

    pub fn level(&self) -> usize {
        self.level
    }

    pub fn zoom(&self) -> f64 {
        self.log_zoom.position.exp()
    }

    pub fn is_locked(&self) -> bool {
        self.is_locked
    }

    pub fn source_size(&self) -> Vec2 {
        self.source
    }

    pub fn source_rect(&self) -> Rect {
        Rect::new(Vec2::ZERO, self.source)
    }

    pub fn viewport(&self) -> Rect {
        Rect::from_center(self.center.position, self.view_size(self.zoom()))
    }

    pub fn target_viewport(&self) -> Rect {
        Rect::from_center(self.center.target, self.view_size(self.log_zoom.target.exp()))
    }

    /// The part of the source the audience can currently see; this is what
    /// the presenter's overlay outlines.
    pub fn visible_source(&self) -> Rect {
        self.viewport().intersect(&self.source_rect()).unwrap_or(self.source_rect())
    }

    /// Starts over for a newly picked source: whole view, level 1.
    pub fn reset_source(&mut self, size: Vec2) {
        if size.x <= 0.0 || size.y <= 0.0 {
            return;
        }
        self.source = size;
        self.level = 1;
        self.has_source = true;
        self.anchor = None;
        self.log_zoom.snap(0.0);
        self.center.snap(size / 2.0);
    }

    /// The same source changed size, e.g. a shared window opened a menu.
    /// Keeps the zoom level and keeps looking at the same pixels.
    pub fn set_source_size(&mut self, size: Vec2) {
        if !self.has_source {
            return self.reset_source(size);
        }
        if size == self.source || size.x <= 0.0 || size.y <= 0.0 {
            return;
        }
        self.source = size;
        let current = self.view_size(self.zoom());
        let target = self.view_size(self.log_zoom.target.exp());
        self.center.position = self.clamp_center(self.center.position, current);
        self.center.target = self.clamp_center(self.center.target, target);
    }

    pub fn set_cursor(&mut self, cursor: Option<Vec2>) {
        self.cursor = cursor;
    }

    pub fn toggle_lock(&mut self) -> bool {
        self.is_locked = !self.is_locked;
        self.is_locked
    }

    pub fn set_level(&mut self, level: usize) {
        let Some(&zoom) = level.checked_sub(1).and_then(|index| self.config.levels.get(index)) else {
            return;
        };
        let zoom = zoom.max(1.0);
        let size = self.view_size(self.zoom());
        let point = self.cursor.filter(|cursor| self.source_rect().contains(*cursor)).unwrap_or(self.center.position);
        let offset = (point - self.center.position).unscale(size);
        let target_size = self.view_size(zoom);
        self.level = level;
        self.anchor = Some(Anchor { point, offset });
        self.log_zoom.target = zoom.ln();
        self.center.target = self.clamp_center(point - offset.scale(target_size), target_size);
    }

    pub fn update(&mut self, dt: f64) -> Rect {
        let dt = dt.clamp(0.0, 0.25);
        self.log_zoom.step(dt);
        let size = self.view_size(self.zoom());
        if let Some(anchor) = self.anchor {
            let center = self.clamp_center(anchor.point - anchor.offset.scale(size), size);
            self.center.position = center;
            self.center.velocity = Vec2::ZERO;
            if self.log_zoom.is_settled(1e-3) {
                self.anchor = None;
                self.log_zoom.snap(self.log_zoom.target);
                self.center.position = self.center.target;
            } else if self.has_left_anchor(anchor, size) {
                self.anchor = None;
            }
        }
        if self.anchor.is_none() {
            self.follow_cursor();
            self.center.step(dt);
            self.keep_cursor_visible(size);
            self.center.position = self.clamp_center(self.center.position, size);
        }
        self.viewport()
    }

    /// Moving the cursor away mid-zoom hands control back to following, so
    /// zooming and then moving straight away never loses the cursor.
    fn has_left_anchor(&self, anchor: Anchor, size: Vec2) -> bool {
        self.cursor.is_some_and(|cursor| {
            let drift = (cursor - anchor.point).unscale(size);
            drift.x.abs() > ANCHOR_RELEASE || drift.y.abs() > ANCHOR_RELEASE
        })
    }

    /// The springs keep motion calm, but a fast cursor would outrun them
    /// and leave the audience's view; past the margin the view is pushed.
    fn keep_cursor_visible(&mut self, size: Vec2) {
        let Some(cursor) = self.cursor else { return };
        if self.is_locked || !self.source_rect().contains(cursor) {
            return;
        }
        let inner = size * (1.0 - 2.0 * EDGE_MARGIN);
        let min = self.center.position - inner / 2.0;
        let max = self.center.position + inner / 2.0;
        let shift = Vec2::new(overflow(cursor.x, min.x, max.x), overflow(cursor.y, min.y, max.y));
        if shift != Vec2::ZERO {
            self.center.position = self.center.position + shift;
        }
    }

    fn follow_cursor(&mut self) {
        let Some(cursor) = self.cursor else {
            return;
        };
        if self.is_locked || !self.source_rect().contains(cursor) {
            return;
        }
        let size = self.view_size(self.log_zoom.target.exp());
        let inner = size * self.config.dead_zone.clamp(0.0, 1.0);
        let min = self.center.target - inner / 2.0;
        let max = self.center.target + inner / 2.0;
        let shift = Vec2::new(overflow(cursor.x, min.x, max.x), overflow(cursor.y, min.y, max.y));
        if shift != Vec2::ZERO {
            self.center.target = self.clamp_center(self.center.target + shift, size);
        }
    }

    /// Viewport size at `zoom`; at 1x it is the smallest output-shaped box
    /// that contains the whole source, so odd-shaped sources letterbox.
    fn view_size(&self, zoom: f64) -> Vec2 {
        let aspect = self.config.output_aspect;
        let base = if self.source.x / self.source.y > aspect {
            Vec2::new(self.source.x, self.source.x / aspect)
        } else {
            Vec2::new(self.source.y * aspect, self.source.y)
        };
        base / zoom
    }

    fn clamp_center(&self, center: Vec2, size: Vec2) -> Vec2 {
        Vec2::new(clamp_axis(center.x, size.x, self.source.x), clamp_axis(center.y, size.y, self.source.y))
    }
}

fn clamp_axis(center: f64, size: f64, extent: f64) -> f64 {
    if size >= extent { extent / 2.0 } else { center.clamp(size / 2.0, extent - size / 2.0) }
}

fn overflow(value: f64, min: f64, max: f64) -> f64 {
    if value < min {
        value - min
    } else if value > max {
        value - max
    } else {
        0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FRAME: f64 = 1.0 / 60.0;

    fn camera(width: f64, height: f64) -> Camera {
        let mut camera = Camera::new(CameraConfig::default());
        camera.reset_source(Vec2::new(width, height));
        camera
    }

    fn settle(camera: &mut Camera) {
        for _ in 0..600 {
            camera.update(FRAME);
        }
    }

    fn assert_close(actual: Vec2, expected: Vec2) {
        assert!(
            (actual.x - expected.x).abs() < 0.5 && (actual.y - expected.y).abs() < 0.5,
            "{actual:?} != {expected:?}"
        );
    }

    #[test]
    fn level_one_shows_whole_matching_source() {
        let mut camera = camera(1920.0, 1080.0);
        let viewport = camera.update(FRAME);
        assert_eq!(viewport, Rect::new(Vec2::ZERO, Vec2::new(1920.0, 1080.0)));
    }

    #[test]
    fn level_one_letterboxes_narrow_source() {
        let mut camera = camera(1200.0, 900.0);
        let viewport = camera.update(FRAME);
        assert_close(viewport.size, Vec2::new(1600.0, 900.0));
        assert_close(viewport.center(), Vec2::new(600.0, 450.0));
        assert_eq!(camera.visible_source(), camera.source_rect());
    }

    #[test]
    fn zoom_keeps_cursor_point_fixed_on_screen() {
        let mut camera = camera(1920.0, 1080.0);
        let cursor = Vec2::new(800.0, 500.0);
        camera.set_cursor(Some(cursor));
        let start = camera.viewport();
        let fraction = (cursor - start.origin).unscale(start.size);
        camera.set_level(3);
        for _ in 0..120 {
            let viewport = camera.update(FRAME);
            let now = (cursor - viewport.origin).unscale(viewport.size);
            assert_close(now * 1000.0, fraction * 1000.0);
        }
        assert!((camera.zoom() - 2.0).abs() < 1e-6);
        assert_close(camera.viewport().size, Vec2::new(960.0, 540.0));
    }

    #[test]
    fn zoom_near_corner_stays_inside_source() {
        let mut camera = camera(1920.0, 1080.0);
        camera.set_cursor(Some(Vec2::new(5.0, 5.0)));
        camera.set_level(5);
        for _ in 0..120 {
            let viewport = camera.update(FRAME);
            assert!(viewport.origin.x >= -1e-6 && viewport.origin.y >= -1e-6);
            assert!(viewport.max().x <= 1920.0 + 1e-6 && viewport.max().y <= 1080.0 + 1e-6);
        }
    }

    #[test]
    fn small_moves_inside_dead_zone_do_not_pan() {
        let mut camera = camera(1920.0, 1080.0);
        camera.set_cursor(Some(Vec2::new(960.0, 540.0)));
        camera.set_level(3);
        settle(&mut camera);
        let before = camera.viewport();
        camera.set_cursor(Some(Vec2::new(1100.0, 600.0)));
        settle(&mut camera);
        assert_eq!(camera.viewport(), before);
    }

    #[test]
    fn leaving_dead_zone_pans_just_enough() {
        let mut camera = camera(1920.0, 1080.0);
        camera.set_cursor(Some(Vec2::new(960.0, 540.0)));
        camera.set_level(3);
        settle(&mut camera);
        camera.set_cursor(Some(Vec2::new(1500.0, 540.0)));
        settle(&mut camera);
        let viewport = camera.viewport();
        let inner_right = viewport.center().x + viewport.size.x * 0.35;
        assert!((inner_right - 1500.0).abs() < 0.5);
    }

    #[test]
    fn lock_freezes_viewport() {
        let mut camera = camera(1920.0, 1080.0);
        camera.set_cursor(Some(Vec2::new(960.0, 540.0)));
        camera.set_level(4);
        settle(&mut camera);
        assert!(camera.toggle_lock());
        let before = camera.viewport();
        camera.set_cursor(Some(Vec2::new(1900.0, 1000.0)));
        settle(&mut camera);
        assert_eq!(camera.viewport(), before);
    }

    #[test]
    fn unknown_level_is_ignored() {
        let mut camera = camera(1920.0, 1080.0);
        camera.set_level(0);
        camera.set_level(42);
        settle(&mut camera);
        assert_eq!(camera.level(), 1);
        assert!((camera.zoom() - 1.0).abs() < 1e-9);
    }

    #[test]
    fn source_resize_keeps_zoom_and_spot() {
        let mut camera = camera(1920.0, 1200.0);
        camera.set_cursor(Some(Vec2::new(600.0, 400.0)));
        camera.set_level(4);
        settle(&mut camera);
        let before = camera.viewport().center();
        camera.set_source_size(Vec2::new(2100.0, 1300.0));
        settle(&mut camera);
        assert_eq!(camera.level(), 4);
        assert!((camera.zoom() - 3.0).abs() < 1e-6);
        assert_close(camera.viewport().center(), before);
    }

    #[test]
    fn new_source_starts_from_whole_view() {
        let mut camera = camera(1920.0, 1080.0);
        camera.set_cursor(Some(Vec2::new(300.0, 300.0)));
        camera.set_level(5);
        settle(&mut camera);
        camera.reset_source(Vec2::new(1920.0, 1080.0));
        assert_eq!(camera.level(), 1);
        assert_eq!(camera.update(FRAME), camera.source_rect());
    }

    #[test]
    fn moving_away_mid_zoom_keeps_the_cursor_in_view() {
        let mut camera = camera(1920.0, 1080.0);
        camera.set_cursor(Some(Vec2::new(960.0, 540.0)));
        camera.set_level(5);
        for _ in 0..6 {
            camera.update(FRAME);
        }
        let cursor = Vec2::new(1500.0, 900.0);
        camera.set_cursor(Some(cursor));
        for _ in 0..60 {
            camera.update(FRAME);
        }
        assert!(camera.viewport().contains(cursor), "{:?} misses {cursor:?}", camera.viewport());
        assert!((camera.zoom() - 4.0).abs() < 1e-3);
    }

    #[test]
    fn fast_cursor_never_leaves_the_view() {
        let mut camera = camera(1920.0, 1080.0);
        camera.set_cursor(Some(Vec2::new(300.0, 200.0)));
        camera.set_level(4);
        settle(&mut camera);
        for step in 0..30 {
            let cursor = Vec2::new(300.0 + step as f64 * 50.0, 200.0 + step as f64 * 25.0);
            camera.set_cursor(Some(cursor));
            camera.update(FRAME);
            assert!(camera.viewport().contains(cursor), "frame {step}: {:?} misses {cursor:?}", camera.viewport());
        }
    }

    #[test]
    fn returning_to_level_one_recenters() {
        let mut camera = camera(1920.0, 1080.0);
        camera.set_cursor(Some(Vec2::new(200.0, 200.0)));
        camera.set_level(5);
        settle(&mut camera);
        camera.set_level(1);
        settle(&mut camera);
        assert_close(camera.viewport().center(), Vec2::new(960.0, 540.0));
    }
}
