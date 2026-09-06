pub fn parse_drive_letter(arg: &str) -> Option<char> {
    let drive = arg.trim().trim_end_matches(':');

    if drive.len() == 1 {
        let letter = drive.chars().next()?.to_ascii_uppercase();
        if letter.is_ascii_alphabetic() {
            return Some(letter);
        }
    }

    None
}

pub fn drive_letter_from_args_or(default: char) -> char {
    std::env::args()
        .nth(1)
        .as_deref()
        .and_then(parse_drive_letter)
        .unwrap_or(default.to_ascii_uppercase())
}
