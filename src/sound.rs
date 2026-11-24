//! Synthesized sound effects and the players that request them.

use std::f64::consts::TAU;
use std::fmt::Debug;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender};
use std::thread;
use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{Device, FromSample, SampleFormat, SizedSample, Stream, StreamConfig};

/// Short cues for distinct game events.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SoundEffect {
    /// A button press.
    Tap,
    /// A correct answer.
    Correct,
    /// An incorrect answer.
    Incorrect,
    /// The end of a round.
    RoundComplete,
}

impl SoundEffect {
    /// Complete set, ordered by mixer slot.
    pub const ALL: [Self; 4] = [
        Self::Tap,
        Self::Correct,
        Self::Incorrect,
        Self::RoundComplete,
    ];

    /// Exact length of the synthesized effect.
    #[must_use]
    pub fn duration(self) -> Duration {
        Duration::from_millis(self.duration_millis())
    }

    const fn duration_millis(self) -> u64 {
        match self {
            Self::Tap => 30,
            Self::Correct => 250,
            Self::Incorrect => 280,
            Self::RoundComplete => 600,
        }
    }

    const fn slot(self) -> usize {
        self as usize
    }
}

/// Requests sound effects on the UI thread. Implementations must return immediately.
pub trait SoundPlayer: Debug {
    /// Requests a cue without waiting for the audio device. The request may be discarded.
    fn play(&self, effect: SoundEffect);
}

/// Discards playback requests without opening an audio device.
#[derive(Debug, Default)]
pub struct SilentPlayer;

impl SoundPlayer for SilentPlayer {
    fn play(&self, _effect: SoundEffect) {}
}

/// Player backed by the system audio device on a dedicated thread.
#[derive(Debug)]
pub struct SystemPlayer {
    requests: SyncSender<SoundEffect>,
}

const QUEUE_CAPACITY: usize = 16;
const IDLE_TIMEOUT: Duration = Duration::from_secs(5);
const ERROR_POLL_INTERVAL: Duration = Duration::from_millis(50);

impl SystemPlayer {
    /// Starts the player, falling back to silence without an output device or on iOS Simulator.
    #[must_use]
    pub fn start() -> Self {
        let (requests, incoming) = mpsc::sync_channel(QUEUE_CAPACITY);
        // Simulator can expose a default device even when RemoteIO initialization
        // aborts the process on an audio service RPC timeout instead of returning an error.
        if cfg!(all(target_os = "ios", target_abi = "sim")) {
            return Self { requests };
        }
        if let Err(error) = thread::Builder::new()
            .name("kunianki-audio".into())
            .spawn(move || {
                run_audio(&incoming, IDLE_TIMEOUT, open_output, |error| {
                    eprintln!("Sound effects unavailable: {error}");
                });
            })
        {
            eprintln!("Sound effects unavailable: {error}");
        }
        Self { requests }
    }
}

impl SoundPlayer for SystemPlayer {
    fn play(&self, effect: SoundEffect) {
        let _ = self.requests.try_send(effect);
    }
}

trait OutputStream {
    fn play(&self) -> Result<(), cpal::Error>;
    fn pause(&self) -> Result<(), cpal::Error>;
}

impl OutputStream for Stream {
    fn play(&self) -> Result<(), cpal::Error> {
        StreamTrait::play(self)
    }

    fn pause(&self) -> Result<(), cpal::Error> {
        StreamTrait::pause(self)
    }
}

fn run_audio<S: OutputStream>(
    incoming: &Receiver<SoundEffect>,
    idle_timeout: Duration,
    open: impl FnOnce(
        Receiver<SoundEffect>,
        SyncSender<cpal::Error>,
    ) -> Result<S, Box<dyn std::error::Error>>,
    report_failure: impl FnOnce(Box<dyn std::error::Error>),
) {
    let (requests, pending) = mpsc::sync_channel(QUEUE_CAPACITY);
    let (errors, terminal_errors) = mpsc::sync_channel(1);
    let check_error = || match terminal_errors.try_recv() {
        Ok(error) => Err(error),
        Err(_) => Ok(()),
    };
    if let Err(error) = open(pending, errors).and_then(|stream| {
        check_error()?;
        stream.play()?;
        let mut paused = false;
        let mut idle_deadline = Some(Instant::now() + idle_timeout);
        loop {
            check_error()?;
            let wait = idle_deadline.map_or(ERROR_POLL_INTERVAL, |deadline| {
                ERROR_POLL_INTERVAL.min(deadline.saturating_duration_since(Instant::now()))
            });
            let received = incoming.recv_timeout(wait);
            check_error()?;
            let effect = match received {
                Ok(effect) => effect,
                Err(RecvTimeoutError::Timeout) => {
                    if idle_deadline.is_some_and(|deadline| Instant::now() >= deadline) {
                        idle_deadline = None;
                        match stream.pause() {
                            Ok(()) => paused = true,
                            Err(error) if error.kind() == cpal::ErrorKind::UnsupportedOperation => {
                                // Keep forwarding without scheduling another pause.
                            }
                            Err(error) => return Err(error.into()),
                        }
                    }
                    continue;
                }
                Err(RecvTimeoutError::Disconnected) => break,
            };
            if paused {
                stream.play()?;
                paused = false;
                idle_deadline = Some(Instant::now() + idle_timeout);
            }
            check_error()?;
            let _ = requests.try_send(effect);
            if idle_deadline.is_some() {
                idle_deadline = Some(Instant::now() + idle_timeout);
            }
        }
        Ok(())
    }) {
        report_failure(error);
        for _ in incoming {}
    }
}

fn open_output(
    pending: Receiver<SoundEffect>,
    errors: SyncSender<cpal::Error>,
) -> Result<Stream, Box<dyn std::error::Error>> {
    #[cfg(target_os = "ios")]
    configure_ios_audio()?;

    let device = cpal::default_host()
        .default_output_device()
        .ok_or("no default audio output device")?;
    let config = device.default_output_config()?;
    let format = config.sample_format();
    let config = config.config();
    if config.channels == 0 || config.sample_rate == 0 {
        return Err("invalid default audio output configuration".into());
    }
    match format {
        SampleFormat::I16 => build_stream::<i16>(&device, config, pending, errors),
        SampleFormat::U16 => build_stream::<u16>(&device, config, pending, errors),
        SampleFormat::F32 => build_stream::<f32>(&device, config, pending, errors),
        _ => return Err(format!("unsupported audio output sample format: {format}").into()),
    }
    .map_err(Into::into)
}

#[cfg(target_os = "ios")]
fn configure_ios_audio() -> Result<(), Box<dyn std::error::Error>> {
    use objc2_avf_audio::{AVAudioSession, AVAudioSessionCategoryAmbient};

    // SAFETY: Ambient is the framework's static category string, valid for the shared session.
    unsafe {
        let category = AVAudioSessionCategoryAmbient.ok_or("iOS Ambient category unavailable")?;
        AVAudioSession::sharedInstance()
            .setCategory_error(category)
            .map_err(|error| format!("could not set iOS Ambient audio category: {error}"))?;
    }
    Ok(())
}

fn build_stream<T: SizedSample + FromSample<f32>>(
    device: &Device,
    config: StreamConfig,
    pending: Receiver<SoundEffect>,
    errors: SyncSender<cpal::Error>,
) -> Result<Stream, cpal::Error> {
    let channels = usize::from(config.channels);
    let mut mixer = Mixer::new(config.sample_rate);
    device.build_output_stream(
        config,
        move |output: &mut [T], _| {
            // Bound callback work even when the UI is continuously producing requests.
            for effect in pending.try_iter().take(QUEUE_CAPACITY) {
                mixer.trigger(effect);
            }
            for frame in output.chunks_mut(channels) {
                frame.fill(T::from_sample(mixer.next_sample()));
            }
        },
        move |error| report_stream_error(&errors, error),
        None,
    )
}

fn report_stream_error(errors: &SyncSender<cpal::Error>, error: cpal::Error) {
    use cpal::ErrorKind::{DeviceChanged, RealtimeDenied, Xrun};

    if !matches!(error.kind(), Xrun | RealtimeDenied | DeviceChanged) {
        let _ = errors.try_send(error);
    }
}

/// Generates mono samples with a length of `round(duration * sample_rate)`.
#[must_use]
#[expect(
    clippy::cast_possible_truncation,
    reason = "convert bounded samples to PCM f32"
)]
pub fn synthesize(effect: SoundEffect, sample_rate: u32) -> Vec<f32> {
    // Note end times are cumulative so rounding never adds or removes a sample.
    type Note = (u64, f64, f64);
    let (notes, peak, harmonic, harmonic_gain, attack): (&[Note], f64, f64, f64, f64) = match effect
    {
        SoundEffect::Tap => (&[(30, 1_800.0, 0.008)], 0.25, 2.0, 0.0, 0.002),
        SoundEffect::Correct => (
            &[(90, 880.0, 0.0), (250, 1_318.5, 0.075)],
            0.5,
            2.0,
            0.3,
            0.005,
        ),
        SoundEffect::Incorrect => (
            &[(120, 196.0, 0.0), (160, 0.0, 0.0), (280, 165.0, 0.0)],
            0.45,
            3.0,
            0.3,
            0.005,
        ),
        SoundEffect::RoundComplete => (
            &[
                (100, 523.25, 0.0),
                (200, 659.25, 0.0),
                (300, 783.99, 0.0),
                (600, 1_046.5, 0.14),
            ],
            0.5,
            2.0,
            0.2,
            0.005,
        ),
    };
    let frames_at = |ms| u32::try_from((ms * u64::from(sample_rate) + 500) / 1_000).unwrap_or(0);
    let mut output = Vec::with_capacity(frames_at(effect.duration_millis()) as usize);
    let rate = f64::from(sample_rate);
    let mut start = 0;
    // Keep phase continuous as the frequency changes. Short releases also ease each note boundary.
    let mut phase: f64 = 0.0;
    for &(end_ms, pitch, decay) in notes {
        let end = frames_at(end_ms);
        let frames = end - start;
        start = end;
        if pitch == 0.0 {
            output.resize(end as usize, 0.0);
            continue;
        }
        let phase_step = TAU * pitch / rate;
        for frame in 0..frames {
            let time = f64::from(frame) / rate;
            let remaining = f64::from(frames - frame - 1) / rate;
            let onset = (time / attack).min(1.0);
            let release = (remaining / 0.005).min(1.0);
            let decay = if decay > 0.0 {
                (-(time - attack).max(0.0) / decay).exp()
            } else {
                1.0
            };
            let tone =
                (phase.sin() + harmonic_gain * (phase * harmonic).sin()) / (1.0 + harmonic_gain);
            output.push((peak * onset * release * decay * tone) as f32);
            phase = (phase + phase_step) % TAU;
        }
    }
    let measured_peak = output.iter().map(|sample| sample.abs()).fold(0.0, f32::max);
    if measured_peak > 0.0 {
        let scale = peak as f32 / measured_peak;
        for sample in &mut output {
            *sample *= scale;
        }
    }
    output
}

/// Fixed-capacity software mixer for use in an audio callback.
#[derive(Debug)]
pub struct Mixer {
    waveforms: [Vec<f32>; Self::MAX_VOICES],
    slots: [VoiceSlot; Self::MAX_VOICES],
    transition_frames: usize,
}

#[derive(Debug, Default)]
struct VoiceSlot {
    cursor: Option<usize>,
    last_sample: f32,
    restart_from: Option<f32>,
}

impl Mixer {
    /// The maximum number of distinct effects that can overlap.
    pub const MAX_VOICES: usize = SoundEffect::ALL.len();

    /// Pre-synthesizes each effect at the given sample rate.
    #[must_use]
    pub fn new(sample_rate: u32) -> Self {
        Self {
            waveforms: SoundEffect::ALL.map(|effect| synthesize(effect, sample_rate)),
            slots: std::array::from_fn(|_| VoiceSlot::default()),
            transition_frames: sample_rate.div_ceil(500) as usize,
        }
    }

    /// Starts or restarts the single voice reserved for this effect.
    ///
    /// Repeated triggers before the first sample is rendered are coalesced.
    /// Effects with no samples are ignored.
    pub fn trigger(&mut self, effect: SoundEffect) {
        let slot = &mut self.slots[effect.slot()];
        match slot.cursor {
            Some(0) => {}
            Some(_) => {
                slot.restart_from = Some(slot.last_sample);
                slot.cursor = Some(0);
            }
            None => {
                slot.restart_from = None;
                slot.cursor = (!self.waveforms[effect.slot()].is_empty()).then_some(0);
            }
        }
    }

    /// Overwrites the mono output buffer with samples clamped to `[-1.0, 1.0]`.
    pub fn render(&mut self, out: &mut [f32]) {
        for sample in out {
            *sample = self.next_sample();
        }
    }

    /// Returns the number of effects still playing.
    #[must_use]
    pub fn active_voices(&self) -> usize {
        self.slots
            .iter()
            .filter(|slot| slot.cursor.is_some())
            .count()
    }

    /// Returns one while this effect is playing, or zero otherwise.
    #[must_use]
    pub fn active_voices_of(&self, effect: SoundEffect) -> usize {
        usize::from(self.slots[effect.slot()].cursor.is_some())
    }

    #[expect(
        clippy::cast_precision_loss,
        reason = "2 ms of u32 sample rates fits exactly in f32"
    )]
    fn next_sample(&mut self) -> f32 {
        let mut mixed: f32 = 0.0;
        for (slot, waveform) in self.slots.iter_mut().zip(&self.waveforms) {
            if let Some(index) = slot.cursor {
                let mut sample = waveform[index];
                if let Some(from) = slot.restart_from {
                    let last = self.transition_frames.min(waveform.len()).saturating_sub(1);
                    if index == 0 {
                        sample = from;
                    } else if index < last {
                        let weight = index as f32 / last as f32;
                        sample = from * (1.0 - weight) + sample * weight;
                    }
                    if index >= last {
                        slot.restart_from = None;
                    }
                }
                mixed += sample;
                slot.last_sample = sample;
                let next = index + 1;
                slot.cursor = (next < waveform.len()).then_some(next);
            }
        }
        mixed.clamp(-1.0, 1.0)
    }
}

#[cfg(test)]
mod tests {
    use std::num::TryFromIntError;

    use super::*;

    const SAMPLE_RATES: [u32; 2] = [44_100, 48_000];
    const MIXER_RATE: u32 = 48_000;

    struct FakeStream {
        events: mpsc::Sender<&'static str>,
        pending: std::sync::Arc<std::sync::Mutex<Receiver<SoundEffect>>>,
        pause_error: Option<cpal::ErrorKind>,
    }

    impl OutputStream for FakeStream {
        fn play(&self) -> Result<(), cpal::Error> {
            assert!(
                matches!(
                    self.pending.lock().map(|pending| pending.try_recv()),
                    Ok(Err(mpsc::TryRecvError::Empty))
                ),
                "play must precede cue forwarding"
            );
            let _ = self.events.send("play");
            Ok(())
        }

        fn pause(&self) -> Result<(), cpal::Error> {
            let _ = self.events.send("pause");
            self.pause_error.map_or(Ok(()), |kind| Err(kind.into()))
        }
    }

    #[test]
    fn worker_forwards_cues_after_idle_with_or_without_pause()
    -> Result<(), Box<dyn std::error::Error>> {
        let idle = Duration::from_millis(20);
        let deadline = Duration::from_secs(2);
        for pause_error in [None, Some(cpal::ErrorKind::UnsupportedOperation)] {
            let (requests, incoming) = mpsc::sync_channel(QUEUE_CAPACITY);
            let (events, observed) = mpsc::channel();
            let (output, opened) = mpsc::channel();
            let worker = thread::spawn(move || {
                run_audio(
                    &incoming,
                    idle,
                    |pending, _| {
                        let pending = std::sync::Arc::new(std::sync::Mutex::new(pending));
                        output.send(pending.clone())?;
                        Ok(FakeStream {
                            events,
                            pending,
                            pause_error,
                        })
                    },
                    |error| panic!("unexpected worker failure: {error}"),
                );
            });
            let pending = opened.recv_timeout(deadline)?;
            assert_eq!(observed.recv_timeout(deadline)?, "play");
            assert_eq!(observed.recv_timeout(deadline)?, "pause");
            requests.send(SoundEffect::Tap)?;
            if pause_error.is_none() {
                assert_eq!(observed.recv_timeout(deadline)?, "play");
            }
            let receive_cue = || {
                let queue = pending.lock().map_err(|_| "pending queue poisoned")?;
                Ok::<_, Box<dyn std::error::Error>>(queue.recv_timeout(deadline)?)
            };
            assert_eq!(receive_cue()?, SoundEffect::Tap);

            if pause_error.is_some() {
                assert_eq!(
                    observed.recv_timeout(idle * 3),
                    Err(RecvTimeoutError::Timeout)
                );
                requests.send(SoundEffect::Correct)?;
                assert_eq!(receive_cue()?, SoundEffect::Correct);
            }
            drop(requests);
            assert!(worker.join().is_ok(), "audio worker panicked");
            if pause_error.is_some() {
                assert_eq!(observed.try_recv(), Err(mpsc::TryRecvError::Disconnected));
            }
        }
        Ok(())
    }

    #[test]
    fn worker_ignores_xrun_but_reports_terminal_errors_during_idle_waits()
    -> Result<(), Box<dyn std::error::Error>> {
        let deadline = Duration::from_secs(2);
        for (idle, pause_error) in [
            (IDLE_TIMEOUT, None),
            (Duration::from_millis(20), None),
            (
                Duration::from_millis(20),
                Some(cpal::ErrorKind::UnsupportedOperation),
            ),
        ] {
            let (requests, incoming) = mpsc::sync_channel(QUEUE_CAPACITY);
            let (events, observed) = mpsc::channel();
            let (output, opened) = mpsc::channel();
            let (failures, reported) = mpsc::channel();
            let worker = thread::spawn(move || {
                run_audio(
                    &incoming,
                    idle,
                    |pending, errors| {
                        let pending = std::sync::Arc::new(std::sync::Mutex::new(pending));
                        output.send((pending.clone(), errors))?;
                        Ok(FakeStream {
                            events,
                            pending,
                            pause_error,
                        })
                    },
                    |error| {
                        let _ = failures
                            .send(error.downcast_ref::<cpal::Error>().map(cpal::Error::kind));
                    },
                );
            });
            let (pending, errors) = opened.recv_timeout(deadline)?;
            assert_eq!(observed.recv_timeout(deadline)?, "play");
            if idle == IDLE_TIMEOUT {
                report_stream_error(&errors, cpal::ErrorKind::Xrun.into());
                requests.send(SoundEffect::Tap)?;
                let queue = pending.lock().map_err(|_| "pending queue poisoned")?;
                assert_eq!(queue.recv_timeout(deadline)?, SoundEffect::Tap);
            } else {
                assert_eq!(observed.recv_timeout(deadline)?, "pause");
            }

            let terminal = cpal::ErrorKind::DeviceNotAvailable;
            report_stream_error(&errors, terminal.into());
            assert_eq!(reported.recv_timeout(deadline)?, Some(terminal));
            // The stream owns the only event sender. It must drop without another cue.
            assert_eq!(
                observed.recv_timeout(deadline),
                Err(RecvTimeoutError::Disconnected)
            );
            requests.send(SoundEffect::Correct)?;
            drop(requests);
            assert!(worker.join().is_ok(), "audio worker panicked");
            assert_eq!(reported.try_recv(), Err(mpsc::TryRecvError::Disconnected));
            let queue = pending.lock().map_err(|_| "pending queue poisoned")?;
            assert_eq!(queue.try_recv(), Err(mpsc::TryRecvError::Disconnected));
        }
        Ok(())
    }

    #[test]
    fn waveforms_have_the_specified_lengths() -> Result<(), TryFromIntError> {
        for (effect, millis) in SoundEffect::ALL.into_iter().zip([30, 250, 280, 600]) {
            let duration = Duration::from_millis(millis);
            assert_eq!(effect.duration(), duration, "{effect:?}");
            for rate in SAMPLE_RATES {
                let expected = usize::try_from((millis * u64::from(rate) + 500) / 1_000)?;
                assert_eq!(
                    synthesize(effect, rate).len(),
                    expected,
                    "{effect:?} at {rate} Hz"
                );
            }
        }
        Ok(())
    }

    #[test]
    fn waveforms_reach_the_specified_peaks() {
        for rate in SAMPLE_RATES {
            for (effect, expected) in [
                (SoundEffect::Tap, 0.25),
                (SoundEffect::Correct, 0.50),
                (SoundEffect::Incorrect, 0.45),
                (SoundEffect::RoundComplete, 0.50),
            ] {
                let measured = synthesize(effect, rate)
                    .into_iter()
                    .map(f32::abs)
                    .fold(0.0, f32::max);
                assert!(
                    (measured - expected).abs() <= 1e-6,
                    "{effect:?} at {rate} Hz: expected peak {expected}, got {measured}"
                );
            }
        }
    }

    #[test]
    fn waveforms_are_audible_bounded_and_continuous() {
        for rate in SAMPLE_RATES {
            for effect in SoundEffect::ALL {
                let samples = synthesize(effect, rate);
                assert!(
                    samples
                        .iter()
                        .all(|sample| sample.is_finite() && sample.abs() <= 0.6),
                    "out-of-range sample: {effect:?} at {rate} Hz"
                );
                assert!(
                    samples.iter().any(|sample| sample.abs() >= 0.1),
                    "inaudible waveform: {effect:?} at {rate} Hz"
                );
                assert!(
                    samples.first().is_some_and(|sample| sample.abs() < 1e-3),
                    "abrupt onset: {effect:?} at {rate} Hz"
                );
                assert!(
                    samples.last().is_some_and(|&sample| sample == 0.0),
                    "abrupt end: {effect:?} at {rate} Hz"
                );
                assert!(
                    samples
                        .windows(2)
                        .all(|pair| (pair[1] - pair[0]).abs() < 0.25),
                    "sample discontinuity: {effect:?} at {rate} Hz"
                );
            }
        }
    }

    fn zero_crossings(samples: &[f32]) -> usize {
        let nonzero = samples.iter().copied().filter(|&sample| sample != 0.0);
        nonzero
            .scan(None, |previous, sample| {
                let sign = sample.is_sign_positive();
                Some(previous.replace(sign).is_some_and(|last| last != sign))
            })
            .filter(|&crossing| crossing)
            .count()
    }

    fn higher_zero_crossing_rate(faster: &[f32], slower: &[f32]) -> bool {
        zero_crossings(faster) * slower.len() > zero_crossings(slower) * faster.len()
    }

    #[test]
    fn waveforms_have_distinct_pitch_profiles() {
        for rate in SAMPLE_RATES {
            let [tap, correct, incorrect, round_complete] =
                SoundEffect::ALL.map(|effect| synthesize(effect, rate));

            assert!(higher_zero_crossing_rate(&tap, &correct));
            assert!(higher_zero_crossing_rate(&correct, &incorrect));

            for rising in [&correct, &round_complete] {
                let quarter = rising.len() / 4;
                assert!(higher_zero_crossing_rate(
                    &rising[rising.len() - quarter..],
                    &rising[..quarter]
                ));
            }
            let (first, second) = incorrect.split_at(incorrect.len() / 2);
            assert!(higher_zero_crossing_rate(first, second));
        }
    }

    #[test]
    fn repeated_taps_use_one_voice() {
        let mut mixer = Mixer::new(MIXER_RATE);
        for _ in 0..100 {
            mixer.trigger(SoundEffect::Tap);
        }
        assert_eq!(mixer.active_voices_of(SoundEffect::Tap), 1);
        assert_eq!(mixer.active_voices(), 1);
    }

    #[test]
    fn repeated_non_taps_use_one_voice_per_distinct_effect() {
        let mut mixer = Mixer::new(MIXER_RATE);
        let effects = [
            SoundEffect::Correct,
            SoundEffect::Incorrect,
            SoundEffect::RoundComplete,
        ];
        for effect in effects.into_iter().cycle().take(6) {
            mixer.trigger(effect);
        }
        // Six requests name only three distinct non-Tap effects. Each repeat restarts its voice.
        assert_eq!(mixer.active_voices(), effects.len());
        for effect in effects {
            assert_eq!(mixer.active_voices_of(effect), 1, "{effect:?}");
        }
    }

    #[test]
    fn active_tap_restarts_continuously_even_during_a_transition() {
        let tap = SoundEffect::Tap;
        let waveform = synthesize(tap, MIXER_RATE);
        let mut mixer = Mixer::new(MIXER_RATE);
        mixer.trigger(tap);
        let mut initial = [0.0; 101];
        mixer.render(&mut initial);
        assert_eq!(initial, waveform[..101]);
        assert!(initial[100].abs() > 0.2);

        mixer.trigger(tap);
        let mut partial_blend = [0.0; 32];
        mixer.render(&mut partial_blend);
        assert_eq!(partial_blend[0], initial[100]);
        assert_ne!(partial_blend[31], waveform[31]);

        mixer.trigger(tap);
        assert_eq!(mixer.active_voices(), 1);
        assert_eq!(mixer.active_voices_of(tap), 1);
        let mut restarted = vec![f32::NAN; waveform.len()];
        mixer.render(&mut restarted);
        assert_eq!(restarted[0], partial_blend[31]);
        assert_eq!(restarted[95..], waveform[95..]);
        assert_eq!(mixer.active_voices(), 0);
        assert_eq!(mixer.next_sample(), 0.0);
    }

    #[test]
    fn mixer_stays_bounded_and_overwrites_output_with_silence_after_drain() {
        let mut mixer = Mixer::new(MIXER_RATE);
        for effect in SoundEffect::ALL.into_iter().chain([SoundEffect::Tap; 4]) {
            mixer.trigger(effect);
            assert!(mixer.active_voices() <= Mixer::MAX_VOICES);
        }
        assert_eq!(mixer.active_voices(), Mixer::MAX_VOICES);
        for effect in SoundEffect::ALL {
            assert_eq!(mixer.active_voices_of(effect), 1, "{effect:?}");
        }

        let mut output = vec![f32::NAN; MIXER_RATE as usize];
        mixer.render(&mut output);
        assert!(output.iter().all(|sample| (-1.0..=1.0).contains(sample)));
        let waveforms = SoundEffect::ALL.map(|effect| synthesize(effect, MIXER_RATE));
        for (frame, mixed) in output.into_iter().enumerate() {
            let sum: f32 = waveforms
                .iter()
                .filter_map(|waveform| waveform.get(frame))
                .sum();
            assert!(
                (mixed - sum.clamp(-1.0, 1.0)).abs() < 1e-6,
                "frame {frame}: mixed {mixed}, unclamped sum {sum}"
            );
        }
        assert_eq!(mixer.active_voices(), 0);

        let mut silence = [f32::NAN; 16];
        mixer.render(&mut silence);
        assert_eq!(silence, [0.0; 16]);
    }

    #[test]
    fn system_player_returns_promptly_with_or_without_an_output_device() {
        let start = Instant::now();
        let player = SystemPlayer::start();
        for _ in 0..1_000 {
            player.play(SoundEffect::Tap);
        }
        assert!(start.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn system_player_worker_drains_and_exits_without_a_default_device() {
        let (requests, incoming) = mpsc::sync_channel(QUEUE_CAPACITY);
        let player = SystemPlayer { requests };
        let worker = thread::spawn(move || {
            run_audio::<FakeStream>(
                &incoming,
                IDLE_TIMEOUT,
                |_, _| Err("no default audio output device".into()),
                |_| {},
            );
        });
        for effect in SoundEffect::ALL {
            player.play(effect);
        }
        drop(player);
        assert!(worker.join().is_ok(), "missing-device fallback panicked");
    }
}
