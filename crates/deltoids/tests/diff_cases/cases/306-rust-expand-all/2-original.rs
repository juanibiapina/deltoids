use std::fmt;

struct Counter {
    value: i32,
}

impl Counter {
    fn new() -> Self {
        Counter { value: 0 }
    }

    fn increment(&mut self) {
        self.value += 1;
    }

    fn describe(&self) -> String {
        format!("{}", self.value)
    }

    fn reset(&mut self) {
        self.value = 0;
    }
}

fn main() {
    let mut counter = Counter::new();
    counter.increment();
}
