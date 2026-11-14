use devices::StartError;
use engine::audio_thread::{AudioThread, Controls};
use engine::driver::OfflineDriver;
use napi::bindgen_prelude::*;
use transport::Network;

use crate::clock::Clock;
use crate::engine::{ClockKind, Engine, OpenOptions, OutputDevice};
use crate::events_thread::Deliver;
use crate::test_os::TestOs;

/// `Engine.open` on libuv's pool: the clock starts there, so Node's event loop runs on while a
/// device's start takes up to the follower's 3 s, and the engine is built back on the control
/// thread when the start returns. A start that fails drops everything the open made with it.
pub struct Opening {
    options: OpenOptions,
    test_os: Option<TestOs>,
    audio: Option<AudioThread>,
    engine: Option<(Controls, Network, Deliver)>,
}

impl Opening {
    pub fn new(
        options: OpenOptions,
        test_os: Option<TestOs>,
        audio: AudioThread,
        controls: Controls,
        network: Network,
        deliver: Deliver,
    ) -> Self {
        Self { options, test_os, audio: Some(audio), engine: Some((controls, network, deliver)) }
    }
}

impl Task for Opening {
    type Output = std::result::Result<Clock, StartError>;
    type JsValue = Engine;

    fn compute(&mut self) -> Result<Self::Output> {
        let audio = self.audio.take().ok_or_else(opened)?;
        Ok(match self.options.clock {
            ClockKind::Offline => {
                Ok(Clock::Offline(Box::new(OfflineDriver::new(audio.into_render()))))
            }
            ClockKind::Timer => Ok(Clock::timer(audio.into_render())?),
            ClockKind::Device => Clock::open(
                self.options.device.take(),
                self.options.period_frames,
                audio,
                self.test_os.take(),
            ),
        })
    }

    fn resolve(&mut self, env: Env, clock: Self::Output) -> Result<Engine> {
        let clock = clock.map_err(|error| start_error(&env, error))?;
        let (controls, network, deliver) = self.engine.take().ok_or_else(opened)?;
        Ok(Engine::running(controls, clock, network, deliver, self.options.playout_delay_frames))
    }
}

// An unavailable output rejects with an error that carries its code and device, so a host tells
// it from every other failure without reading the message.
fn start_error(env: &Env, error: StartError) -> Error {
    let message = error.to_string();
    let StartError::Unavailable { output, .. } = error else {
        return Error::from_reason(message);
    };
    env.create_error(Error::from_reason(message))
        .and_then(|mut js| {
            js.set_named_property("code", "output-unavailable")?;
            let device = output.map(|output| OutputDevice { id: output.id, name: output.name });
            js.set_named_property("device", device)?;
            Ok(Error::from(js.to_unknown()))
        })
        .unwrap_or_else(|error| error)
}

fn opened() -> Error {
    Error::from_reason("this open has run already")
}
