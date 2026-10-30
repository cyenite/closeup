use std::ops::{Add, Mul, Sub};

/// Critically damped spring, integrated with the closed-form solution so it
/// stays stable and never overshoots regardless of frame time.
#[derive(Clone, Copy, Debug)]
pub struct Spring<T> {
    pub omega: f64,
    pub target: T,
    pub velocity: T,
    pub position: T,
}

impl<T> Spring<T>
where
    T: Copy + Add<Output = T> + Sub<Output = T> + Mul<f64, Output = T>,
{
    pub fn new(position: T, omega: f64) -> Self {
        Self { omega, target: position, velocity: position * 0.0, position }
    }

    pub fn snap(&mut self, value: T) {
        self.target = value;
        self.position = value;
        self.velocity = value * 0.0;
    }

    pub fn step(&mut self, dt: f64) {
        let offset = self.position - self.target;
        let decay = (-self.omega * dt).exp();
        let slope = self.velocity + offset * self.omega;
        self.position = self.target + (offset + slope * dt) * decay;
        self.velocity = (self.velocity - slope * (self.omega * dt)) * decay;
    }
}

impl Spring<f64> {
    pub fn is_settled(&self, epsilon: f64) -> bool {
        (self.position - self.target).abs() < epsilon && self.velocity.abs() < epsilon
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converges_without_overshoot() {
        let mut spring = Spring::new(0.0, 12.0);
        spring.target = 1.0;
        let mut previous = 0.0;
        for _ in 0..240 {
            spring.step(1.0 / 60.0);
            assert!(spring.position >= previous - 1e-12);
            assert!(spring.position <= 1.0 + 1e-12);
            previous = spring.position;
        }
        assert!(spring.is_settled(1e-4));
    }

    #[test]
    fn frame_rate_independent() {
        let mut fast = Spring::new(0.0, 10.0);
        let mut slow = Spring::new(0.0, 10.0);
        fast.target = 1.0;
        slow.target = 1.0;
        for _ in 0..120 {
            fast.step(1.0 / 120.0);
        }
        for _ in 0..30 {
            slow.step(1.0 / 30.0);
        }
        assert!((fast.position - slow.position).abs() < 1e-9);
    }
}
