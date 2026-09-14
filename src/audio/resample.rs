use crate::timeline::MIX_SAMPLE_RATE;

pub struct LinearResampler {
    step: f64,
    src_pos: f64,
    buffered: Vec<[f32; 2]>,
}

impl LinearResampler {
    pub fn new(src_hz: u32) -> Self {
        let src_hz = src_hz.max(1);
        Self {
            step: f64::from(src_hz) / f64::from(MIX_SAMPLE_RATE),
            src_pos: 0.0,
            buffered: Vec::new(),
        }
    }

    pub fn push(&mut self, frames: &[[f32; 2]], out: &mut Vec<[f32; 2]>) {
        if frames.is_empty() {
            return;
        }
        if (self.step - 1.0).abs() < f64::EPSILON {
            out.extend_from_slice(frames);
            return;
        }
        self.buffered.extend_from_slice(frames);
        loop {
            let idx = self.src_pos.floor() as usize;
            let next = idx + 1;
            if next >= self.buffered.len() {
                break;
            }
            let frac = (self.src_pos - idx as f64) as f32;
            let a = self.buffered[idx];
            let b = self.buffered[next];
            out.push([a[0] + (b[0] - a[0]) * frac, a[1] + (b[1] - a[1]) * frac]);
            self.src_pos += self.step;
        }
        let keep_from = (self.src_pos.floor() as usize).min(self.buffered.len().saturating_sub(1));
        if keep_from > 0 {
            self.buffered.drain(..keep_from);
            self.src_pos -= keep_from as f64;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::PI;

    fn sine(hz: u32, frames: usize, freq: f32) -> Vec<[f32; 2]> {
        (0..frames)
            .map(|i| {
                let s = (i as f32 / hz as f32 * freq * 2.0 * PI).sin() * 0.5;
                [s, s]
            })
            .collect()
    }

    fn zero_crossings(frames: &[[f32; 2]]) -> usize {
        frames
            .windows(2)
            .filter(|pair| pair[0][0] <= 0.0 && pair[1][0] > 0.0)
            .count()
    }

    fn resample_all(src_hz: u32, frames: &[[f32; 2]]) -> Vec<[f32; 2]> {
        let mut resampler = LinearResampler::new(src_hz);
        let mut out = Vec::new();
        resampler.push(frames, &mut out);
        out
    }

    #[test]
    fn passthrough_at_mix_rate_is_identity() {
        let input = [[0.1, -0.2], [0.3, 0.4]];
        assert_eq!(resample_all(MIX_SAMPLE_RATE, &input), input);
    }

    #[test]
    fn sixteen_khz_second_becomes_one_mix_second_at_the_same_pitch() {
        let src_hz = 16_000;
        let input = sine(src_hz, src_hz as usize, 440.0);
        let out = resample_all(src_hz, &input);
        let expected = MIX_SAMPLE_RATE as usize;
        assert!(
            (out.len() as i32 - expected as i32).abs() <= 3,
            "16 kHz -> 48 kHz produced {} frames, expected {expected}",
            out.len()
        );
        let src_cross = zero_crossings(&input);
        let dst_cross = zero_crossings(&out);
        assert!(
            (dst_cross as i32 - src_cross as i32).abs() <= 2,
            "pitch moved: {src_hz} Hz had {src_cross} crossings, mix rate had {dst_cross}"
        );
        assert!(
            (src_cross as i32 - 440).abs() <= 2,
            "fixture was not 440 Hz: {src_cross} crossings"
        );
    }

    #[test]
    fn forty_four_one_keeps_duration() {
        let src_hz = 44_100;
        let input = sine(src_hz, src_hz as usize, 440.0);
        let out = resample_all(src_hz, &input);
        let secs = out.len() as f64 / f64::from(MIX_SAMPLE_RATE);
        assert!(
            (secs - 1.0).abs() < 0.002,
            "44.1 kHz -> 48 kHz lasted {secs} s"
        );
    }

    #[test]
    fn packets_match_one_shot() {
        let src_hz = 16_000;
        let input = sine(src_hz, 1_600, 440.0);
        let one_shot = resample_all(src_hz, &input);
        let mut streamed = LinearResampler::new(src_hz);
        let mut out = Vec::new();
        for chunk in input.chunks(160) {
            streamed.push(chunk, &mut out);
        }
        let n = out.len().min(one_shot.len());
        assert!(
            (out.len() as i32 - one_shot.len() as i32).abs() <= 2,
            "streamed {} frames, one-shot {}",
            out.len(),
            one_shot.len()
        );
        for (got, want) in out[..n].iter().zip(&one_shot[..n]) {
            assert!((got[0] - want[0]).abs() < 1e-5);
            assert!((got[1] - want[1]).abs() < 1e-5);
        }
    }
}
