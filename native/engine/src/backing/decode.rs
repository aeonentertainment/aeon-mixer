use std::fs::File;
use std::io::Cursor;

use rubato::audioadapter_buffers::direct::SequentialSliceOfVecs;
use rubato::{Fft, FixedSync, Resampler};
use symphonia::core::codecs::audio::AudioDecoderOptions;
use symphonia::core::errors::Error;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, TrackType};
use symphonia::core::io::{MediaSource, MediaSourceStream};
use symphonia::core::meta::MetadataOptions;

use crate::backing::{BackingError, Planar, StemSource};
use crate::block::RATE;

/// Frames per resampler call; the whole file goes through, so this only sizes its FFTs.
const RESAMPLE_CHUNK: usize = 1024;

/// Decodes one stem with Symphonia, a pure Rust decoder, to stereo at 48 kHz. Mono plays on both
/// sides, channels past the second are dropped, and any other rate is resampled. A failure names
/// the file's path, or for bytes the stem's `kind` as the host calls it. Bytes are probed by their
/// content and freed as the decode ends. Runs on the worker thread.
pub fn decode(source: StemSource, kind: &str) -> Result<Planar, BackingError> {
    let mut hint = Hint::new();
    let (stem, media): (String, Result<Box<dyn MediaSource>, String>) = match source {
        StemSource::File(path) => {
            if let Some(extension) = path.extension().and_then(|e| e.to_str()) {
                hint.with_extension(extension);
            }
            let file = File::open(&path).map(|file| Box::new(file) as Box<dyn MediaSource>);
            (path.display().to_string(), file.map_err(|e| e.to_string()))
        }
        StemSource::Bytes(bytes) => {
            (format!("the {kind} stem's bytes"), Ok(Box::new(Cursor::new(bytes))))
        }
    };
    let fail = |reason: String| BackingError::Decode { stem: stem.clone(), reason };
    let stream = MediaSourceStream::new(media.map_err(fail)?, Default::default());
    let mut format = symphonia::default::get_probe()
        .probe(&hint, stream, FormatOptions::default(), MetadataOptions::default())
        .map_err(|e| fail(e.to_string()))?;
    let track =
        format.default_track(TrackType::Audio).ok_or_else(|| fail("no audio track".to_owned()))?;
    let track_id = track.id;
    let codec = track
        .codec_params
        .as_ref()
        .and_then(|params| params.audio())
        .ok_or_else(|| fail("no audio codec parameters".to_owned()))?;
    let mut decoder = symphonia::default::get_codecs()
        .make_audio_decoder(codec, &AudioDecoderOptions::default())
        .map_err(|e| fail(e.to_string()))?;

    let mut audio = Planar::default();
    let mut rate = RATE;
    let mut planes: Vec<Vec<f32>> = Vec::new();
    while let Some(packet) = format.next_packet().map_err(|e| fail(e.to_string()))? {
        if packet.track_id != track_id {
            continue;
        }
        let decoded = match decoder.decode(&packet) {
            Ok(decoded) => decoded,
            // A corrupt packet is skipped, as players do; the song plays on
            Err(Error::DecodeError(_)) => continue,
            Err(e) => return Err(fail(e.to_string())),
        };
        rate = decoded.spec().rate();
        decoded.copy_to_vecs_planar(&mut planes);
        let Some(left) = planes.first() else { continue };
        let right = planes.get(1).unwrap_or(left);
        audio.left.extend_from_slice(left);
        audio.right.extend_from_slice(right);
    }
    if rate == RATE || audio.frames() == 0 {
        return Ok(audio);
    }
    resample(audio, rate).map_err(fail)
}

/// The whole file through Rubato's FFT resampler, which trims its own delay.
fn resample(audio: Planar, from: u32) -> Result<Planar, String> {
    let frames = audio.frames();
    let mut resampler =
        Fft::<f32>::new(from as usize, RATE as usize, RESAMPLE_CHUNK, 2, FixedSync::Input)
            .map_err(|e| e.to_string())?;
    let input = [audio.left, audio.right];
    let capacity = resampler.process_all_needed_output_len(frames);
    let mut output = [vec![0.0; capacity], vec![0.0; capacity]];
    let (_, written) = resampler
        .process_all_into_buffer(
            &SequentialSliceOfVecs::new(&input, 2, frames).map_err(|e| e.to_string())?,
            &mut SequentialSliceOfVecs::new_mut(&mut output, 2, capacity)
                .map_err(|e| e.to_string())?,
            frames,
            None,
        )
        .map_err(|e| e.to_string())?;
    let [mut left, mut right] = output;
    left.truncate(written);
    right.truncate(written);
    Ok(Planar { left, right })
}
