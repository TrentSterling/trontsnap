// The synthesized shutter sound, shared by both platform sound backends
// (sound.rs plays it via winmm, linux/sound.rs via paplay/aplay). No bundled
// asset: a short filtered-noise burst shaped by a fast exponential-decay
// envelope (two "curtain" clicks plus a low thud), 44.1kHz 16-bit mono PCM,
// ~110ms, emitted as a complete in-memory WAV.

/// Synthesize a crisp two-click shutter sound: 44.1kHz, 16-bit, mono, ~110ms.
pub fn build_shutter_wav() -> Vec<u8> {
    const SAMPLE_RATE: u32 = 44_100;
    const DURATION_MS: u32 = 110;
    let n = (SAMPLE_RATE * DURATION_MS / 1000) as usize;

    // Deterministic xorshift32 PRNG — no `rand` dependency needed for one buffer.
    let mut rng_state: u32 = 0x9E3779B9;
    let mut next_noise = move || -> f32 {
        rng_state ^= rng_state << 13;
        rng_state ^= rng_state >> 17;
        rng_state ^= rng_state << 5;
        (rng_state as f32 / u32::MAX as f32) * 2.0 - 1.0
    };

    // One-pole high-pass: y[n] = a*(y[n-1] + x[n] - x[n-1]).
    let mut hp_prev_in = 0.0f32;
    let mut hp_prev_out = 0.0f32;
    let hp_alpha = 0.90f32;

    // (start_sec, amplitude, decay_tau_sec) — first curtain, second curtain.
    let clicks: [(f32, f32, f32); 2] = [(0.0, 0.9, 0.016), (0.042, 0.55, 0.010)];

    let mut samples = vec![0i16; n];
    for i in 0..n {
        let t = i as f32 / SAMPLE_RATE as f32;

        let raw = next_noise();
        let hp = hp_alpha * (hp_prev_out + raw - hp_prev_in);
        hp_prev_in = raw;
        hp_prev_out = hp;

        let mut acc = 0.0f32;
        for (start, amp, tau) in clicks {
            if t >= start {
                let dt = t - start;
                let env = (-dt / tau).exp();
                acc += hp * env * amp;
            }
        }

        // Low body under the first click (mechanical "thud").
        let thud_env = (-t / 0.02).exp();
        let thud = (2.0 * std::f32::consts::PI * 140.0 * t).sin() * thud_env * 0.25;

        let mixed = (acc + thud).clamp(-1.0, 1.0);
        samples[i] = (mixed * i16::MAX as f32) as i16;
    }

    encode_wav_pcm16_mono(&samples, SAMPLE_RATE)
}

/// Build a minimal valid RIFF/WAVE/PCM header + data, 16-bit mono.
fn encode_wav_pcm16_mono(samples: &[i16], sample_rate: u32) -> Vec<u8> {
    let channels: u16 = 1;
    let bits_per_sample: u16 = 16;
    let byte_rate = sample_rate * channels as u32 * bits_per_sample as u32 / 8;
    let block_align = channels * bits_per_sample / 8;
    let data_len = (samples.len() * 2) as u32;
    let riff_len = 36 + data_len;

    let mut buf = Vec::with_capacity(44 + data_len as usize);
    buf.extend_from_slice(b"RIFF");
    buf.extend_from_slice(&riff_len.to_le_bytes());
    buf.extend_from_slice(b"WAVE");
    buf.extend_from_slice(b"fmt ");
    buf.extend_from_slice(&16u32.to_le_bytes()); // fmt chunk size (PCM)
    buf.extend_from_slice(&1u16.to_le_bytes()); // format tag: PCM
    buf.extend_from_slice(&channels.to_le_bytes());
    buf.extend_from_slice(&sample_rate.to_le_bytes());
    buf.extend_from_slice(&byte_rate.to_le_bytes());
    buf.extend_from_slice(&block_align.to_le_bytes());
    buf.extend_from_slice(&bits_per_sample.to_le_bytes());
    buf.extend_from_slice(b"data");
    buf.extend_from_slice(&data_len.to_le_bytes());
    for s in samples {
        buf.extend_from_slice(&s.to_le_bytes());
    }
    buf
}
