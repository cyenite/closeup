use std::path::PathBuf;

use closeup_core::CameraConfig;
use serde::Deserialize;

#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
pub struct Config {
    pub levels: Vec<f64>,
    pub fps: u32,
    pub pan_omega: f64,
    pub dead_zone: f64,
    pub zoom_omega: f64,
    pub cursor_scale: f64,
}

impl Default for Config {
    fn default() -> Self {
        let camera = CameraConfig::default();
        Self {
            levels: camera.levels,
            fps: 60,
            pan_omega: camera.pan_omega,
            dead_zone: camera.dead_zone,
            zoom_omega: camera.zoom_omega,
            cursor_scale: 1.0,
        }
    }
}

impl Config {
    pub fn load() -> Self {
        let Some(path) = dirs::config_dir().map(|dir| dir.join("closeup/config.toml")) else {
            return Self::default();
        };
        let Ok(text) = std::fs::read_to_string(&path) else {
            return Self::default();
        };
        match toml::from_str::<Self>(&text) {
            Ok(config) => config.validated(),
            Err(error) => {
                log::warn!("ignoring {}: {error}", path.display());
                Self::default()
            }
        }
    }

    /// Clamps every value into a range the camera handles well, warning
    /// about each one that had to change.
    pub fn validated(mut self) -> Self {
        let defaults = Self::default();
        let levels: Vec<f64> =
            self.levels.iter().copied().filter(|zoom| zoom.is_finite() && *zoom >= 1.0).take(9).collect();
        if levels.len() != self.levels.len() {
            log::warn!("config: levels must be 1.0 or more, at most 9 of them; using {levels:?}");
        }
        self.levels = if levels.is_empty() { defaults.levels } else { levels };
        self.fps = clamped("fps", self.fps, 10, 240);
        self.pan_omega = clamped("pan_omega", self.pan_omega, 1.0, 60.0);
        self.dead_zone = clamped("dead_zone", self.dead_zone, 0.0, 0.95);
        self.zoom_omega = clamped("zoom_omega", self.zoom_omega, 1.0, 60.0);
        self.cursor_scale = clamped("cursor_scale", self.cursor_scale, 0.25, 4.0);
        self
    }

    pub fn camera(&self) -> CameraConfig {
        CameraConfig {
            levels: self.levels.clone(),
            pan_omega: self.pan_omega,
            dead_zone: self.dead_zone,
            zoom_omega: self.zoom_omega,
            ..CameraConfig::default()
        }
    }
}

pub fn restore_token_path() -> PathBuf {
    dirs::state_dir().or_else(dirs::data_local_dir).unwrap_or_else(std::env::temp_dir).join("closeup/restore-token")
}

fn clamped<T: PartialOrd + Copy + std::fmt::Debug>(name: &str, value: T, min: T, max: T) -> T {
    let result = if value < min {
        min
    } else if value > max {
        max
    } else {
        value
    };
    if result != value {
        log::warn!("config: {name} = {value:?} is outside {min:?}..={max:?}; using {result:?}");
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> Config {
        toml::from_str::<Config>(text).unwrap().validated()
    }

    #[test]
    fn missing_keys_fall_back_to_defaults() {
        let config = parse("fps = 30");
        assert_eq!(config.fps, 30);
        assert_eq!(config.levels, Config::default().levels);
    }

    #[test]
    fn out_of_range_values_are_clamped() {
        let config = parse("fps = 1000\ndead_zone = 2.0\ncursor_scale = 0.0\npan_omega = -3.0");
        assert_eq!(config.fps, 240);
        assert_eq!(config.dead_zone, 0.95);
        assert_eq!(config.cursor_scale, 0.25);
        assert_eq!(config.pan_omega, 1.0);
    }

    #[test]
    fn invalid_levels_are_dropped() {
        let config = parse("levels = [1.0, 0.5, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 10.0]");
        assert_eq!(config.levels, vec![1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0]);
        assert_eq!(parse("levels = [0.1]").levels, Config::default().levels);
    }
}
