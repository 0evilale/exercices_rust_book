fn main() {
    let fahrenheit: f32 = 98.6;
    let result = fahrenheit_to_celcius(fahrenheit);
    assert_eq!(37_f32, result);
    println!("{result}");
}

fn fahrenheit_to_celcius(fahrenheit: f32) -> f32 {
    ((fahrenheit - 32_f32) * 5_f32) / 9_f32
}
