use std::time::Instant;
use vterm_emulator::TerminalSize;
use vterm_emulator::Terminal;

fn main() {
    // 0. Resize reflow: 10k-line scrollback, alternating widths (issue #1).
    let mut term = Terminal::new(TerminalSize::new(50, 200));
    let mut data = Vec::new();
    for i in 0..12_000u32 {
        data.extend_from_slice(
            format!("history {i:06} lorem ipsum dolor sit amet consectetur adipiscing elit padding padding padding padding padding padding\r\n")
                .as_bytes(),
        );
    }
    term.process(&data);
    let start = Instant::now();
    let mut wide = true;
    for _ in 0..20 {
        let cols = if wide { 120 } else { 200 };
        term.resize(TerminalSize::new(50, cols));
        wide = !wide;
    }
    println!(
        "resize-reflow: 20 resizes with ~10k-line scrollback in {:?} ({:.1} ms/resize)",
        start.elapsed(),
        start.elapsed().as_secs_f64() * 1000.0 / 20.0
    );

    // 1. Scroll-heavy stream: plain lines, one scroll_up per line once full.
    let mut term = Terminal::new(TerminalSize::new(50, 200));
    let mut data = Vec::new();
    for i in 0..200_000u32 {
        data.extend_from_slice(
            format!("line {i:06} lorem ipsum dolor sit amet consectetur adipiscing elit padding padding padding padding padding padding padding\r\n")
                .as_bytes(),
        );
    }
    let start = Instant::now();
    for chunk in data.chunks(4096) {
        term.process(chunk);
    }
    let elapsed = start.elapsed();
    println!(
        "scroll-heavy: {:.1} MB in {:?} = {:.1} MB/s",
        data.len() as f64 / 1e6,
        elapsed,
        data.len() as f64 / 1e6 / elapsed.as_secs_f64()
    );

    // 2. TUI redraw: full-screen colored frame repeated (codex-style).
    let mut term = Terminal::new(TerminalSize::new(50, 200));
    let mut frame = Vec::new();
    frame.extend_from_slice(b"\x1b[H");
    for r in 0..50u32 {
        frame.extend_from_slice(
            format!(
                "\x1b[38;5;{}m\x1b[48;5;{}mrow {:03} colored text with cjk 你好世界 and padding to fill the line out towards two hundred columns of total width abcdefghij\x1b[0m\x1b[K\r\n",
                r % 256,
                (r * 7) % 256,
                r
            )
            .as_bytes(),
        );
    }
    let frames = 3000u32;
    let start = Instant::now();
    for _ in 0..frames {
        term.process(&frame);
    }
    let elapsed = start.elapsed();
    let total = frame.len() as f64 * frames as f64 / 1e6;
    println!(
        "tui-redraw:   {:.1} MB in {:?} = {:.1} MB/s ({:.0} full frames/s)",
        total,
        elapsed,
        total / elapsed.as_secs_f64(),
        frames as f64 / elapsed.as_secs_f64()
    );
}
