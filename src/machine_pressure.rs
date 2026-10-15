//! Battery/thermal admission hints for background work (research **J004**).

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PowerSource {
    #[default]
    Unknown,
    Ac,
    Battery,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ThermalState {
    #[default]
    Nominal,
    Fair,
    Serious,
    Critical,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PressureSnapshot {
    pub power: PowerSource,
    pub battery_percent: Option<u8>,
    pub thermal: ThermalState,
    pub low_power_mode: bool,
}

impl PressureSnapshot {
    pub fn allows_background_preview(self) -> bool {
        !matches!(self.thermal, ThermalState::Serious | ThermalState::Critical)
            && !(self.low_power_mode && matches!(self.power, PowerSource::Battery))
            && self.battery_percent.is_none_or(|percent| percent >= 20)
    }

    pub fn allows_background_index(self) -> bool {
        self.allows_background_preview()
            && !matches!(self.thermal, ThermalState::Fair)
            && self.battery_percent.is_none_or(|percent| percent >= 30)
    }

    pub fn allows_background_hash(self) -> bool {
        self.allows_background_index()
    }
}

pub fn snapshot() -> PressureSnapshot {
    if let Ok(raw) = std::env::var("COMMANDER_MACHINE_PRESSURE") {
        return parse_test_snapshot(&raw).unwrap_or_default();
    }
    PressureSnapshot::default()
}

fn parse_test_snapshot(raw: &str) -> Option<PressureSnapshot> {
    let mut snap = PressureSnapshot::default();
    for part in raw.split(';') {
        let (key, value) = part.split_once('=')?;
        match key.trim() {
            "power" => {
                snap.power = match value.trim() {
                    "ac" => PowerSource::Ac,
                    "battery" => PowerSource::Battery,
                    _ => PowerSource::Unknown,
                };
            }
            "battery" => snap.battery_percent = value.trim().parse().ok(),
            "thermal" => {
                snap.thermal = match value.trim() {
                    "fair" => ThermalState::Fair,
                    "serious" => ThermalState::Serious,
                    "critical" => ThermalState::Critical,
                    _ => ThermalState::Nominal,
                };
            }
            "low_power" => snap.low_power_mode = matches!(value.trim(), "1" | "true"),
            _ => {}
        }
    }
    Some(snap)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn low_battery_blocks_background_preview_and_index() {
        let snap = PressureSnapshot {
            power: PowerSource::Battery,
            battery_percent: Some(10),
            thermal: ThermalState::Nominal,
            low_power_mode: false,
        };
        assert!(!snap.allows_background_preview());
        assert!(!snap.allows_background_index());
        assert!(!snap.allows_background_hash());
        let fair = PressureSnapshot {
            power: PowerSource::Ac,
            battery_percent: Some(80),
            thermal: ThermalState::Fair,
            low_power_mode: false,
        };
        assert!(fair.allows_background_preview());
        assert!(!fair.allows_background_index());
    }

    fn on_battery(percent: u8) -> PressureSnapshot {
        PressureSnapshot {
            power: PowerSource::Battery,
            battery_percent: Some(percent),
            ..PressureSnapshot::default()
        }
    }

    #[test]
    fn battery_thresholds_split_preview_from_index() {
        assert!(!on_battery(19).allows_background_preview());
        assert!(on_battery(20).allows_background_preview());
        assert!(!on_battery(20).allows_background_index());
        assert!(on_battery(25).allows_background_preview());
        assert!(!on_battery(29).allows_background_index());
        assert!(!on_battery(29).allows_background_hash());
        assert!(on_battery(30).allows_background_index());
        assert!(on_battery(30).allows_background_hash());
    }

    #[test]
    fn low_power_mode_blocks_only_on_battery() {
        let low_power = |power| PressureSnapshot {
            power,
            low_power_mode: true,
            ..PressureSnapshot::default()
        };
        let battery = low_power(PowerSource::Battery);
        assert!(!battery.allows_background_preview());
        assert!(!battery.allows_background_index());
        for power in [PowerSource::Ac, PowerSource::Unknown] {
            assert!(low_power(power).allows_background_preview());
            assert!(low_power(power).allows_background_index());
        }
    }

    #[test]
    fn serious_and_critical_thermal_block_everything_on_ac() {
        for thermal in [ThermalState::Serious, ThermalState::Critical] {
            let snap = PressureSnapshot {
                power: PowerSource::Ac,
                battery_percent: Some(100),
                thermal,
                low_power_mode: false,
            };
            assert!(!snap.allows_background_preview(), "{thermal:?}");
            assert!(!snap.allows_background_index(), "{thermal:?}");
            assert!(!snap.allows_background_hash(), "{thermal:?}");
        }
    }
}
