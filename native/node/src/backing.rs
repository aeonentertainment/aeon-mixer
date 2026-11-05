//! The backing path as JavaScript arms it: the render job `createMixer` starts on the worker
//! thread, which the events thread watches for its progress, and the stems it yields, which
//! `Mixer::install_backing` moves into the graph before `start` swaps it in.

use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use engine::backing::{Backing, RenderJob, StemFiles};
use engine::params::StemGains;
use napi::bindgen_prelude::*;
use napi_derive::napi;

#[napi(object)]
pub struct BackingInput {
    pub instrumental: String,
    pub guide: Option<String>,
    /// The armed key, rendered before start.
    pub semitones: i32,
    /// The armed speed factor, rendered before start.
    pub speed: f64,
}

#[napi(object)]
pub struct StemGainsInput {
    pub instrumental: f64,
    pub guide: f64,
}

impl From<StemGainsInput> for StemGains {
    fn from(input: StemGainsInput) -> Self {
        Self { instrumental: input.instrumental as f32, guide: input.guide as f32 }
    }
}

/// The job as the control thread and the events thread share it: `None` once its stems are taken.
pub type SharedRenderJob = Arc<Mutex<Option<RenderJob>>>;

pub fn lock(job: &SharedRenderJob) -> MutexGuard<'_, Option<RenderJob>> {
    job.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A song's stems being decoded and rendered for one mixer. JavaScript holds it from
/// `createMixer` and installs it when the events thread says the job is done.
#[napi]
pub struct BackingRender {
    id: u32,
    job: SharedRenderJob,
    armed_semitones: i8,
    failed: Option<String>,
}

impl BackingRender {
    /// Starts the job on the worker thread. `id` is what the events thread's progress events carry.
    pub(crate) fn start(id: u32, input: BackingInput) -> Result<Self> {
        let semitones = i8::try_from(input.semitones)
            .map_err(|_| Error::from_reason("semitones must be between -128 and 127"))?;
        let speed = input.speed as f32;
        if !(speed.is_finite() && speed > 0.0) {
            return Err(Error::from_reason(format!(
                "speed {} is not a positive factor",
                input.speed
            )));
        }
        let files = StemFiles {
            instrumental: PathBuf::from(input.instrumental),
            guide: input.guide.map(PathBuf::from),
        };
        let job = Arc::new(Mutex::new(Some(RenderJob::start(files, semitones, speed))));
        Ok(Self { id, job, armed_semitones: semitones, failed: None })
    }

    pub(crate) fn job(&self) -> SharedRenderJob {
        Arc::clone(&self.job)
    }

    /// The rendered stems as a backing path, once. `src/` calls this after the events thread saw
    /// the worker finish, so the join reaps the thread without blocking; a failed render is the
    /// engine's message, repeated on every later call.
    pub(crate) fn take_backing(&mut self) -> Result<Backing> {
        if let Some(reason) = &self.failed {
            return Err(Error::from_reason(reason.clone()));
        }
        let job = lock(&self.job)
            .take()
            .ok_or_else(|| Error::from_reason("this backing is already installed"))?;
        match job.finish() {
            Ok(stems) => Ok(Backing::new(stems, self.armed_semitones)),
            Err(error) => {
                let reason = error.to_string();
                self.failed = Some(reason.clone());
                Err(Error::from_reason(reason))
            }
        }
    }
}

#[napi]
impl BackingRender {
    /// The id the events thread's `renderProgress` and `renderFinished` events carry.
    #[napi(getter)]
    pub fn id(&self) -> u32 {
        self.id
    }
}
