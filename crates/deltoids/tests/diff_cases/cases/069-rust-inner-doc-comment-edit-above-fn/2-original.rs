//! Parses the config file.

fn parse(input: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in input.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        out.push(line.to_string());
    }
    out
}
