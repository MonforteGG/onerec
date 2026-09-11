pub const MIX_QUANTUM_FRAMES: usize = 480;
pub const MAX_BACKLOG_FRAMES: usize = 12_000;
pub const MIX_TICK: std::time::Duration = std::time::Duration::from_millis(10);
pub const MIX_SAMPLE_RATE: u32 = 48_000;

const _: () = assert!(
    MIX_SAMPLE_RATE as u64 * MIX_TICK.as_millis() as u64 == MIX_QUANTUM_FRAMES as u64 * 1_000
);

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
