/// Example Rust module with an intentionally slow summation.
fn slow_sum(n: u64) -> u64 {
    let mut total = 0u64;
    let mut i = 0u64;
    while i < n {
        total += i;
        i += 1;
    }
    total
}

/// Idiomatic fast equivalent.
fn fast_sum(n: u64) -> u64 {
    (0..n).sum()
}

fn main() {
    let x = slow_sum(1000);
    println!("{x}");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slow_matches_fast() {
        assert_eq!(slow_sum(100), fast_sum(100));
    }
}
