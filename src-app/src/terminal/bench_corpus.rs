pub(crate) const CORPUS_SEED: u64 = 0x5041_4e45_464c_4f57;
const CORPUS_FAMILIES: usize = 27;
const CORPUS_VARIANTS: usize = 5;
const CORPUS_SIZE: usize = CORPUS_FAMILIES * CORPUS_VARIANTS;

pub(crate) fn deterministic_streams() -> Vec<Vec<u8>> {
    let mut streams = Vec::with_capacity(CORPUS_SIZE);
    for index in 0..CORPUS_SIZE {
        let variant = index / CORPUS_FAMILIES;
        let family = index % CORPUS_FAMILIES;
        let bytes = match family {
            0 => format!("plain-ascii-{variant}\r\n").into_bytes(),
            1 => format!("unicode-{variant}: café Καλημέρα हिन्दी 🦀\r\n").into_bytes(),
            2 => format!("grapheme-{variant}: e\u{301} n\u{303} 👨‍👩‍👧‍👦\r\n").into_bytes(),
            3 => format!("wide-{variant}: 中文 日本語 한글\r\n").into_bytes(),
            4 => format!("\x1b[1;3;4;9mstyled-{variant}\x1b[0m\r\n").into_bytes(),
            5 => format!(
                "\x1b[38;2;{};{};{}mtruecolor-{variant}\x1b[0m",
                20 + variant,
                80 + variant,
                140 + variant
            )
            .into_bytes(),
            6 => format!(
                "origin\x1b[{};{}Hcursor-{variant}\x1b[2A\x1b[3C",
                2 + variant,
                3 + variant
            )
            .into_bytes(),
            7 => (format!("wrap-{variant}-") + &"x".repeat(180 + variant)).into_bytes(),
            8 => (format!("reflow-{variant}-") + &"0123456789".repeat(24)).into_bytes(),
            9 => format!("before\x1b[?1049halt-{variant}\x1b[?1049lafter").into_bytes(),
            10 => (0..40)
                .map(|line| format!("scroll-{variant}-{line}\r\n"))
                .collect::<String>()
                .into_bytes(),
            11 => format!("\x1b[?1h\x1b[?1000h\x1b[?1006hmode-{variant}").into_bytes(),
            12 => format!("\x1b]2;synthetic-title-{variant}\x07title-body").into_bytes(),
            13 => format!("query-{variant}\x1b[5n\x1b[6n\x1b[c\x1b[>c").into_bytes(),
            14 => format!("malformed-{variant}\x1b[999999999999999999999;?;mend").into_bytes(),
            15 => {
                format!("truncated-{variant}\x1b]8;;https://synthetic.invalid/unterminated")
                    .into_bytes()
            }
            16 => format!("erase-{variant}\x1b[2J\x1b[Hredrawn-{variant}").into_bytes(),
            17 => format!(
                "\x1b]8;id=synthetic-{variant};https://example.invalid/{variant}\x07link\x1b]8;;\x07"
            )
            .into_bytes(),
            18 => format!(
                "\x1b]133;A\x07prompt-{variant}\x1b]133;B\x07command\x1b]133;C\x07output\x1b]133;D;0\x07"
            )
            .into_bytes(),
            19 => format!("\x1b]52;c;c3ludGhldGljLWNsaXBib2FyZC0{variant}=\x07").into_bytes(),
            20 => format!(
                "\x1b[{};{}mansi16-{variant}\x1b[0m",
                30 + variant,
                40 + ((variant + 2) % 6)
            )
            .into_bytes(),
            21 => format!(
                "\x1b[38;5;{};48;5;{}mindexed256-{variant}\x1b[0m",
                16 + variant * 17,
                231 - variant * 11
            )
            .into_bytes(),
            22 => format!("\x1b[2;7mdim-inverse-{variant}\x1b[0m").into_bytes(),
            23 => format!("\x1b[{} qcursor-shape-{variant}", variant + 1).into_bytes(),
            24 => {
                let mut bytes = format!("invalid-utf8-{variant}:").into_bytes();
                bytes.extend_from_slice(&[0xf0, 0x28, 0x8c, 0x28, b'\r', b'\n']);
                bytes
            }
            25 => format!("tabs-{variant}:\talpha\t中\tomega\r\n").into_bytes(),
            26 => format!("selection-{variant}-target").into_bytes(),
            _ => unreachable!(),
        };
        streams.push(bytes);
    }
    streams
}
