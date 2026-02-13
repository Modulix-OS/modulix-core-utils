pub fn count_char_before_newline(text: &str, mut initial_pos: usize) -> usize {
    initial_pos += 1;
    let mut number_char = 0;
    while initial_pos > 0 && text.chars().nth(initial_pos-1).unwrap_or('\n') != '\n' {
        initial_pos -= 1;
        number_char += 1;
    }
    number_char
}
