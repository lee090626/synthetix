// A month's bill with proven rules: `axiom build --emit rust` in examples/invoice,
// then `rustc --edition 2024 app/main.rs -o bill && ./bill`.
#[path = "../dist/contracts.rs"]
mod contracts;

use contracts::{fee, payout, prorate, supply_of, tax_of};

fn main() -> Result<(), contracts::AxiomError> {
    let plan = 49_000; // 월 요금 (원)
    let charged = prorate(plan, 12, 31)?; // 31일 중 12일 사용
    let rate = 330; // PG 수수료 3.3%

    println!("청구액 {charged}");
    println!("공급가액 {} 세액 {}", supply_of(charged)?, tax_of(charged)?);
    println!("PG 수수료 {} 정산액 {}", fee(charged, rate)?, payout(charged, rate)?);

    // 쓴 날이 달의 날수보다 많음: need 줄이 막음
    println!("거부: {:?}", prorate(plan, 40, 31));
    Ok(())
}
