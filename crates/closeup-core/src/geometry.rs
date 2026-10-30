use std::ops::{Add, Div, Mul, Sub};

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Vec2 {
    pub x: f64,
    pub y: f64,
}

impl Vec2 {
    pub const ZERO: Self = Self::new(0.0, 0.0);

    pub const fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }

    pub fn scale(self, other: Self) -> Self {
        Self::new(self.x * other.x, self.y * other.y)
    }

    pub fn unscale(self, other: Self) -> Self {
        Self::new(self.x / other.x, self.y / other.y)
    }
}

impl Add for Vec2 {
    type Output = Self;

    fn add(self, other: Self) -> Self {
        Self::new(self.x + other.x, self.y + other.y)
    }
}

impl Sub for Vec2 {
    type Output = Self;

    fn sub(self, other: Self) -> Self {
        Self::new(self.x - other.x, self.y - other.y)
    }
}

impl Mul<f64> for Vec2 {
    type Output = Self;

    fn mul(self, factor: f64) -> Self {
        Self::new(self.x * factor, self.y * factor)
    }
}

impl Div<f64> for Vec2 {
    type Output = Self;

    fn div(self, divisor: f64) -> Self {
        Self::new(self.x / divisor, self.y / divisor)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    pub origin: Vec2,
    pub size: Vec2,
}

impl Rect {
    pub const fn new(origin: Vec2, size: Vec2) -> Self {
        Self { origin, size }
    }

    pub fn from_center(center: Vec2, size: Vec2) -> Self {
        Self::new(center - size / 2.0, size)
    }

    pub fn max(&self) -> Vec2 {
        self.origin + self.size
    }

    pub fn center(&self) -> Vec2 {
        self.origin + self.size / 2.0
    }

    pub fn contains(&self, point: Vec2) -> bool {
        let max = self.max();
        point.x >= self.origin.x && point.y >= self.origin.y && point.x <= max.x && point.y <= max.y
    }

    pub fn intersect(&self, other: &Rect) -> Option<Rect> {
        let min = Vec2::new(self.origin.x.max(other.origin.x), self.origin.y.max(other.origin.y));
        let max = Vec2::new(self.max().x.min(other.max().x), self.max().y.min(other.max().y));
        (max.x > min.x && max.y > min.y).then(|| Rect::new(min, max - min))
    }
}
