pub const MIX_QUANTUM_FRAMES: usize = 480;
pub const MAX_BACKLOG_FRAMES: usize = 12_000;
pub const MIX_TICK: std::time::Duration = std::time::Duration::from_millis(10);
pub const MIX_SAMPLE_RATE: u32 = 48_000;

const _: () = assert!(
    MIX_SAMPLE_RATE as u64 * MIX_TICK.as_millis() as u64 == MIX_QUANTUM_FRAMES as u64 * 1_000
);

/// Average `factor` mix frames into the staging PCM layout and append LE f32 bytes.
///
/// Meeting and Voice are mono: each output sample is the mean of both channels
/// over `factor` mix frames (6 at 8 kHz, 3 at 16 kHz). Compact and above stay
/// 48 kHz stereo (`factor` 1, two samples per frame).
pub fn fold_mix(frames: &[[f32; 2]], factor: usize, channels: u8, out: &mut Vec<u8>) {
    debug_assert!(factor > 0);
    debug_assert_eq!(frames.len() % factor, 0);
    out.clear();
    match channels {
        1 => {
            let n = 2.0 * factor as f32;
            for chunk in frames.chunks_exact(factor) {
                let acc: f32 = chunk.iter().map(|[left, right]| left + right).sum();
                out.extend_from_slice(&(acc / n).clamp(-1.0, 1.0).to_le_bytes());
            }
        }
        2 if factor == 1 => {
            for [left, right] in frames {
                out.extend_from_slice(&left.to_le_bytes());
                out.extend_from_slice(&right.to_le_bytes());
            }
        }
        2 => {
            let n = factor as f32;
            for chunk in frames.chunks_exact(factor) {
                let (left, right) = chunk
                    .iter()
                    .fold((0.0, 0.0), |(l, r), [ml, mr]| (l + *ml, r + *mr));
                out.extend_from_slice(&(left / n).clamp(-1.0, 1.0).to_le_bytes());
                out.extend_from_slice(&(right / n).clamp(-1.0, 1.0).to_le_bytes());
            }
        }
        _ => debug_assert!(false, "staging is mono or stereo"),
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Draw {
    pub discard: usize,
    pub take: usize,
    pub pad: usize,
}

pub fn draw(available: usize, want: usize, max_backlog: usize) -> Draw {
    let out = if available >= want {
        let leftover = available - want;
        let discard = if leftover > max_backlog {
            (leftover - max_backlog).min(want / 8)
        } else {
            0
        };
        Draw {
            discard,
            take: want,
            pad: 0,
        }
    } else {
        Draw {
            discard: 0,
            take: available,
            pad: want - available,
        }
    };
    debug_assert_eq!(out.take + out.pad, want);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn draw_pads_when_short() {
        assert_eq!(
            draw(10, 40, 100),
            Draw {
                discard: 0,
                take: 10,
                pad: 30,
            }
        );
    }

    #[test]
    fn draw_takes_want_when_enough() {
        assert_eq!(
            draw(50, 40, 20),
            Draw {
                discard: 0,
                take: 40,
                pad: 0,
            }
        );
        assert_eq!(
            draw(40, 40, 0),
            Draw {
                discard: 0,
                take: 40,
                pad: 0,
            }
        );
    }

    #[test]
    fn draw_sheds_when_backlog_exceeds_max() {
        assert_eq!(
            draw(100, 40, 10),
            Draw {
                discard: 5,
                take: 40,
                pad: 0,
            }
        );
        assert_eq!(
            draw(55, 40, 10),
            Draw {
                discard: 5,
                take: 40,
                pad: 0,
            }
        );
        assert_eq!(
            draw(1000, 40, 10),
            Draw {
                discard: 5,
                take: 40,
                pad: 0,
            }
        );
    }

    #[test]
    fn fold_mix_meeting_averages_six_stereo_frames_to_one_mono() {
        let mut frames = [[0.0; 2]; 6];
        frames[0] = [1.0, 1.0];
        frames[1] = [-1.0, -1.0];
        let mut out = Vec::new();
        fold_mix(&frames, 6, 1, &mut out);
        assert_eq!(out.len(), 4);
        assert_eq!(f32::from_le_bytes(out.try_into().unwrap()), 0.0);
    }

    #[test]
    fn fold_mix_stereo_passthrough_keeps_both_channels() {
        let mut out = Vec::new();
        fold_mix(&[[0.25, -0.5]], 1, 2, &mut out);
        assert_eq!(out.len(), 8);
        assert_eq!(f32::from_le_bytes(out[..4].try_into().unwrap()), 0.25);
        assert_eq!(f32::from_le_bytes(out[4..].try_into().unwrap()), -0.5);
    }

    #[test]
    fn take_plus_pad_always_equals_want() {
        let cases = [
            (0, 0, 0),
            (0, 40, 10),
            (10, 40, 10),
            (40, 40, 10),
            (50, 40, 20),
            (50, 40, 5),
            (100, 40, 10),
            (7, 7, 0),
            (8, 7, 0),
            (3, 480, 12_000),
            (20_000, 480, 12_000),
            (13_000, 480, 12_000),
        ];
        for (available, want, max_backlog) in cases {
            let drawn = draw(available, want, max_backlog);
            assert_eq!(
                drawn.take + drawn.pad,
                want,
                "draw({available}, {want}, {max_backlog}) = {drawn:?}"
            );
        }
    }
}
