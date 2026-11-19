//! The tone stage judged against a reference vocal mic (decision 0029, docs/design.md 8.9). A
//! slot whose bundle holds `reference/<n>.wav` is a paired take: its slot is replayed alone, with
//! no stems, once with its stage off and once as it ran, lined up with the reference by level, and
//! each take read on the reference's moments of singing, s and room.

mod classes;
mod frame;
mod gap;
mod heard;
mod line_up;
mod pair;
mod reference;

#[cfg(test)]
mod tests;

use std::collections::BTreeMap;
use std::path::Path;

use engine::block::RATE;
use replay::bundle::Bundle;
use replay::bundle::line::{Effect, Event};
use replay::bundle::report::{self, Options as Replay};
use replay::bundle::song;

use classes::Classes;
use frame::Frame;
use heard::Heard;
use line_up::LineUp;
use pair::Pair;
use reference::Reference;

/// The paired rows' numbers by name, and what to print of them for a person.
#[derive(Default)]
pub struct Paired {
    pub numbers: BTreeMap<String, f64>,
    pub lines: Vec<String>,
}

/// The paired rows of the bundle `name` in `dir`, replayed under `profile` or its own. A bundle
/// with no `reference/` scores none and says so.
pub fn score(name: &str, dir: &Path, profile: Option<&Path>) -> Result<Paired, String> {
    let mut paired = Paired::default();
    let references = Reference::find(dir)?;
    if references.is_empty() {
        paired.lines.push(format!(
            "{name}: no reference mic in reference/, so the paired rows were not scored"
        ));
    }
    for reference in references {
        let slot = reference.slot;
        let (on, off) = takes(dir, profile, slot)?;
        let mut header = format!(
            "{name} slot {slot}, against {} at {} cm",
            reference.mic, reference.distance_cm
        );
        let line_up = match LineUp::find(&off, &reference.samples) {
            Ok(line_up) => line_up,
            Err(best) => {
                paired.lines.push(format!(
                    "{header}: the mic's recording does not line up with the slot's take within 60 s \
                     (their levels correlate {best:.2} at best, under the 0.5 trusted), so its paired \
                     rows were not scored"
                ));
                continue;
            }
        };
        let start = (-line_up.lag).max(0) as usize;
        let end = off.len().min((reference.samples.len() as i64 - line_up.lag) as usize);
        let mic = &reference.samples[(start as i64 + line_up.lag) as usize..][..end - start];
        let mic_frames = Frame::all(mic);
        let Some(classes) = Classes::of(&mic_frames) else {
            paired.lines.push(format!(
                "{header}: nothing in the mic's recording reads as singing, so its paired rows were \
                 not scored"
            ));
            continue;
        };
        let hear =
            |take: &[f32]| Heard::of(&take[start..end], &Frame::all(&take[start..end]), &classes);
        let pair = Pair {
            mic: Heard::of(mic, &mic_frames, &classes),
            off: hear(&off),
            on: on.as_deref().map(hear),
        };
        let seconds = line_up.lag.abs() as f64 / f64::from(RATE);
        let when = if line_up.lag >= 0 { "before" } else { "after" };
        header += &format!(
            ", its recording starting {seconds:.2} s {when} the take (levels correlate {:.2})",
            line_up.correlation
        );
        paired.lines.push(header);
        paired.lines.extend(pair.table());
        paired.numbers.extend(pair.numbers(slot));
    }
    Ok(paired)
}

/// Slot `slot`'s take alone, with no stems, as the room heard it, summed to mono: as it ran, when
/// its stage was ever on, and with its stage off.
fn takes(
    dir: &Path,
    profile: Option<&Path>,
    slot: usize,
) -> Result<(Option<Vec<f32>>, Vec<f32>), String> {
    let options = Replay { profile: profile.map(Path::to_path_buf), backing: None };
    let (mut bundle, profile, _) = report::load(dir, &options)?;
    if !bundle.manifest.slots.get(slot).is_some_and(|recorded| recorded.recorded) {
        return Err(format!(
            "reference/{slot}.wav pairs with slot {slot}, which the bundle did not record"
        ));
    }
    bundle.arrivals.retain(|&(index, _)| index == slot);
    let mono = |bundle: &Bundle| -> Result<Vec<f32>, String> {
        let mix = song::play(bundle, &profile, None)?.mix;
        Ok(mix.as_chunks::<2>().0.iter().map(|[l, r]| 0.5 * (l + r)).collect())
    };
    let on =
        if stages(&mut bundle, slot).iter().any(|on| **on) { Some(mono(&bundle)?) } else { None };
    for on in stages(&mut bundle, slot) {
        *on = false;
    }
    Ok((on, mono(&bundle)?))
}

/// Whether each tone effect set on `slot` is on, to read or to turn off.
fn stages(bundle: &mut Bundle, slot: usize) -> Vec<&mut bool> {
    let effects = bundle.events.iter_mut().filter_map(|(_, event)| match event {
        Event::SetSlotEffects { index, effects } if *index == slot => Some(effects),
        _ => None,
    });
    effects
        .flatten()
        .filter_map(|effect| match effect {
            Effect::Tone { enabled, .. } => Some(enabled),
            _ => None,
        })
        .collect()
}
