//! Trace SGR state at every byte offset.
use std::env;
use vterm_emulator::CellAttributes;
use vterm_emulator::Terminal;

fn main() {
    let path = env::args().nth(1).expect("usage: trace <raw>");
    let bytes = std::fs::read(&path).unwrap();
    let mut term = Terminal::new(vterm_emulator::TerminalSize::new(43, 113));
    println!("offset\tbyte\tcursor_y\tcursor_x\tattrs\tfg\tbg");
    for (i, &b) in bytes.iter().enumerate() {
        term.process(&[b]);
        let (cy, cx) = term.cursor_pos();
        let attrs = term.current_attributes();
        let fg = term.current_fg();
        let bg = term.current_bg();
        let bmaj = if (0x20..=0x7e).contains(&b) {
            b as char
        } else {
            '.'
        };
        println!(
            "{}\t{}({})\t{}\t{}\t{:?}\t{:?}\t{:?}",
            i, bmaj, b, cy, cx, attrs, fg, bg
        );
    }
}
