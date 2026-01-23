use freminal_buffer::buffer::Buffer;
use freminal_common::buffer_states::tchar::TChar;
use proptest::prelude::*;

fn c(ch: char) -> TChar {
    TChar::new_from_single_char(ch as u8)
}

proptest! {
    #[test]
    fn decstbm_random_ops_do_not_panic(
        width in 2usize..15,
        height in 2usize..15,
        actions in prop::collection::vec(0u8..=255, 10..200),
    ) {
        let mut buf = Buffer::new(width, height);

        // Warm up the buffer with some content.
        for _ in 0..height {
            let line_len = width.clamp(1, 6);
            let line: Vec<TChar> = (0..line_len).map(|_| c('X')).collect();
            buf.insert_text(&line);
            buf.handle_lf();
        }

        for a in actions {
            // Random DECSTBM region.
            let top = (a as usize % height) + 1;
            let bottom = ((a as usize * 3) % height) + 1;

            if top < bottom {
                buf.set_scroll_region(top, bottom);
            } else {
                buf.set_scroll_region(1, height);
            }

            // Random op.
            match a % 6 {
                0 => buf.handle_lf(),
                1 => buf.handle_ri(),
                2 => buf.handle_ind(),
                3 => buf.handle_nel(),
                4 => buf.insert_lines((a as usize % 3) + 1),
                5 => buf.delete_lines((a as usize % 3) + 1),
                _ => unreachable!(),
            }

            // We *don't* call private invariants. If anything goes wrong,
            // we rely on panics inside your methods or out-of-bounds.
        }

        // Sanity: visible rows never exceed height.
        assert!(buf.visible_rows().len() <= height);
    }
}
