use fastpos_core::{
    format_money, format_quantity, CartItem, CheckoutRequest, Database, NewProduct, PaymentInput,
    PaymentMethod,
};
use std::error::Error;

fn main() -> Result<(), Box<dyn Error>> {
    let mut db = Database::open("fastpos-demo.sqlite3")?;
    let product = match db.find_by_barcode("6111234567890")? {
        Some(existing) => existing,
        None => db.add_product(NewProduct::new(
            "Water 1.5L",
            Some("6111234567890".to_string()),
            1_200,  // 1.200 TND
            10_000, // 10 units
        ))?,
    };

    let receipt = db.checkout(CheckoutRequest {
        items: vec![CartItem {
            product_id: product.id,
            quantity_milli: 2_000,
        }],
        discount_minor: 0,
        payments: vec![PaymentInput {
            method: PaymentMethod::Cash,
            tendered_minor: 3_000,
        }],
    })?;

    println!("Receipt: {}", receipt.receipt_number);
    println!(
        "{} x {}",
        receipt.items[0].product_name,
        format_quantity(receipt.items[0].quantity_milli)
    );
    println!(
        "Total: {} {}",
        receipt.currency_code,
        format_money(receipt.total_minor, receipt.currency_scale)?
    );
    println!(
        "Change: {}",
        format_money(receipt.change_due_minor, receipt.currency_scale)?
    );
    Ok(())
}
