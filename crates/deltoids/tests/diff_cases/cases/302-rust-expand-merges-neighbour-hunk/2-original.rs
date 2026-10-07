struct Shape {
    width: u32,
    height: u32,
}

impl Shape {
    fn area(&self) -> u32 {
        self.width * self.height
    }

    fn describe(&self) -> String {
        let area = self.area();
        let label = "shape";
        let size = if area > 100 { "large" } else { "small" };
        let text = format!("{label} {size}");
        let padded = format!("[{text}]");
        let trimmed = padded.trim().to_string();
        trimmed
    }

    fn perimeter(&self) -> u32 {
        2 * (self.width + self.height)
    }
}
