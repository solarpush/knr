use knr_core::Effect;
use knr_runtime::{HostPlatform, HwPlatform, NetPlatform, RuntimeError, UiPlatform, Value};

pub struct CliHostPlatform {
    net: Option<NetPlatform>,
    ui: UiPlatform,
    hw: HwPlatform,
}

impl Default for CliHostPlatform {
    fn default() -> Self {
        Self::new()
    }
}

impl CliHostPlatform {
    pub fn new() -> Self {
        Self {
            net: None,
            ui: UiPlatform::new(),
            hw: HwPlatform::new(),
        }
    }

    fn ensure_net(&mut self) -> Result<&mut NetPlatform, RuntimeError> {
        if self.net.is_none() {
            self.net = Some(NetPlatform::new()?);
        }
        Ok(self.net.as_mut().expect("net must be initialized"))
    }
}

impl HostPlatform for CliHostPlatform {
    fn host_call(
        &mut self,
        effect: Effect,
        op: &str,
        payload: Value,
    ) -> Result<Value, RuntimeError> {
        match effect {
            Effect::Net | Effect::Async => {
                let net = self.ensure_net()?;
                net.host_call(effect, op, payload)
            }
            Effect::Ui => self.ui.host_call(effect, op, payload),
            Effect::Hw => self.hw.host_call(effect, op, payload),
        }
    }
}
