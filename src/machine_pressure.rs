//! Battery/thermal admission hints for background work (research **J004**).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PowerSource { #[default] Unknown, Ac, Battery }
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ThermalState { #[default] Nominal, Fair, Serious, Critical }

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
            && self.battery_percent.is_none_or(|p| p >= 20)
    }
    pub fn allows_background_index(self) -> bool {
        self.allows_background_preview()
            && !matches!(self.thermal, ThermalState::Fair)
            && self.battery_percent.is_none_or(|p| p >= 30)
    }
    pub fn allows_background_hash(self) -> bool { self.allows_background_index() }
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
        let (k, v) = part.split_once('=')?;
        match k.trim() {
            "power" => snap.power = match v.trim() { "ac" => PowerSource::Ac, "battery" => PowerSource::Battery, _ => PowerSource::Unknown },
            "battery" => snap.battery_percent = v.trim().parse().ok(),
            "thermal" => snap.thermal = match v.trim() { "fair" => ThermalState::Fair, "serious" => ThermalState::Serious, "critical" => ThermalState::Critical, _ => ThermalState::Nominal },
            "low_power" => snap.low_power_mode = matches!(v.trim(), "1" | "true"),
            _ => {}
        }
    }
    Some(snap)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn low_battery_blocks_background_preview() {
        let snap = PressureSnapshot { power: PowerSource::Battery, battery_percent: Some(10), thermal: ThermalState::Nominal, low_power_mode: false };
        assert!(!snap.allows_background_preview());
    }
}
