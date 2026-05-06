pub fn parse(input: &str) -> Vec<String> {
    input.split(',').map(str::to_string).collect()
}
