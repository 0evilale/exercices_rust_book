

fn main() {
    let result_fsr = fibonacci_sequence_recursive(16u32, 0u32, 1u32);
    assert_eq!(987_u32, result_fsr);
    println!("recursive: {result_fsr}");

    let result = fibonacci_sequence(16u16);
    assert_eq!(987_u16, result);
    println!("non-recursive: {result}");
}

fn fibonacci_sequence(num_iter: u16) -> u16{
    let mut after: u16 = 0;
    let mut next: u16 = 1;
    for _ in 1..num_iter {
        let buff_next = after + next;
        after = next;
        next = buff_next;
    }

    return next;
}

fn fibonacci_sequence_recursive(num_iter: u32, start: u32, next: u32) -> u32 {
    if num_iter == 1 {
        return next;
    }
    fibonacci_sequence_recursive(num_iter - 1, next, start + next)
}

