// Compute a transaction service fee in cents.
//
// Pricing rules (per the product spec):
//   - a flat base fee of $5.00 (500 cents)
//   - plus a variable 2% of the transaction amount
//   - the total is capped at $50.00 (5000 cents)
//   - subscribers receive a 25% discount on the final fee
//
// Port of compute_fee.cpp.
fn compute_fee(amount_cents: u32, is_subscriber: bool) -> u32 {
    // Base fee plus the variable portion.
    let mut fee: u32 = 500;
    fee += amount_cents * 2 / 100;

    // Subscribers get 25% off.
    if is_subscriber {
        fee = fee * 75 / 100;
    }

    // Apply the $50 cap.
    if fee > 5000 {
        fee = 5000;
    }
    fee
}
