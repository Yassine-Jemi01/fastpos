use fastpos_core::{
    format_money, format_quantity, CartItem, CheckoutRequest, Database, NewProduct, PaymentInput,
    PaymentMethod,
};
use std::error::Error;
use std::process::ExitCode;

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), Box<dyn Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() || args[0] == "help" || args[0] == "--help" || args[0] == "-h" {
        print_help();
        return Ok(());
    }
    match args[0].as_str() {
        "init" => {
            if !(2..=3).contains(&args.len()) {
                return Err(
                    "wrong number of arguments. Usage: fastpos init <database-file> [country-code]"
                        .into(),
                );
            }
            let mut db = Database::open(&args[1])?;
            if args.len() == 3 {
                db.set_country(&args[2])?;
            }
            let (currency, scale) = db.currency()?;
            let country = db.country()?.unwrap_or_else(|| "not set".to_string());
            println!("FastPOS database ready: {} (country {country}, currency {currency}, scale {scale})", args[1]);
        }
        "set-country" => {
            require_len(&args, 3, "set-country <database-file> <country-code>")?;
            let mut db = Database::open(&args[1])?;
            db.set_country(&args[2])?;
            let (currency, scale) = db.currency()?;
            let country = db.country()?.unwrap_or_else(|| "not set".to_string());
            println!("Store country set to {country}; currency {currency}, scale {scale}.");
        }
        "set-currency" => {
            require_len(&args, 3, "set-currency <database-file> <currency-code>")?;
            let mut db = Database::open(&args[1])?;
            db.set_currency_code(&args[2])?;
            let (currency, scale) = db.currency()?;
            println!("Store currency set to {currency}, scale {scale} (country not set).");
        }
        "currency" => {
            require_len(&args, 2, "currency <database-file>")?;
            let db = Database::open(&args[1])?;
            let (currency, scale) = db.currency()?;
            let country = db.country()?.unwrap_or_else(|| "not set".to_string());
            println!("Country: {country}; currency: {currency}; scale: {scale}");
        }
        "add-product" => {
            require_len(
                &args,
                6,
                "add-product <database-file> <name> <barcode-or-dash> <price-minor> <stock-milli>",
            )?;
            let mut db = Database::open(&args[1])?;
            let barcode = if args[3] == "-" {
                None
            } else {
                Some(args[3].clone())
            };
            let price = parse_i64(&args[4], "price-minor")?;
            let stock = parse_i64(&args[5], "stock-milli")?;
            let product = db.add_product(NewProduct::new(&args[2], barcode, price, stock))?;
            let (_, scale) = db.currency()?;
            println!("Added product #{}: {}", product.id, product.name);
            println!(
                "  Price: {}",
                format_money(product.unit_price_minor, scale)?
            );
            println!("  Stock: {}", format_quantity(product.stock_quantity_milli));
        }
        "products" => {
            require_len(&args, 2, "products <database-file>")?;
            let db = Database::open(&args[1])?;
            let (_, scale) = db.currency()?;
            let products = db.list_products()?;
            if products.is_empty() {
                println!("No products yet.");
            }
            for product in products {
                println!(
                    "#{:<5} {:<28} price={} stock={} {}{}",
                    product.id,
                    truncate(&product.name, 28),
                    format_money(product.unit_price_minor, scale)?,
                    format_quantity(product.stock_quantity_milli),
                    product
                        .barcode
                        .as_deref()
                        .map(|value| format!("barcode={value} "))
                        .unwrap_or_default(),
                    if product.is_active { "" } else { "[inactive]" }
                );
            }
        }
        "sell" => {
            require_len(
                &args,
                5,
                "sell <database-file> <product-id> <quantity-milli> <cash-received-minor>",
            )?;
            let mut db = Database::open(&args[1])?;
            let product_id = parse_i64(&args[2], "product-id")?;
            let quantity = parse_i64(&args[3], "quantity-milli")?;
            let cash = parse_i64(&args[4], "cash-received-minor")?;
            let receipt = db.checkout(CheckoutRequest {
                items: vec![CartItem {
                    product_id,
                    quantity_milli: quantity,
                }],
                discount_minor: 0,
                payments: vec![PaymentInput {
                    method: PaymentMethod::Cash,
                    tendered_minor: cash,
                }],
            })?;
            println!("Receipt {}", receipt.receipt_number);
            println!(
                "  Item: {} x {}",
                receipt.items[0].product_name,
                format_quantity(receipt.items[0].quantity_milli)
            );
            println!(
                "  Total: {} {}",
                receipt.currency_code,
                format_money(receipt.total_minor, receipt.currency_scale)?
            );
            println!(
                "  Cash received: {}",
                format_money(receipt.payments[0].tendered_minor, receipt.currency_scale)?
            );
            println!(
                "  Change due: {}",
                format_money(receipt.change_due_minor, receipt.currency_scale)?
            );
        }
        "sale" => {
            require_len(&args, 3, "sale <database-file> <sale-id>")?;
            let db = Database::open(&args[1])?;
            let id = parse_i64(&args[2], "sale-id")?;
            let receipt = db.get_sale(id)?;
            println!("Receipt {} — {}", receipt.receipt_number, receipt.sold_at);
            for item in &receipt.items {
                println!(
                    "  {} x {} = {}",
                    item.product_name,
                    format_quantity(item.quantity_milli),
                    format_money(item.line_total_minor, receipt.currency_scale)?
                );
            }
            println!(
                "  Subtotal: {}",
                format_money(receipt.subtotal_minor, receipt.currency_scale)?
            );
            println!(
                "  Discount: {}",
                format_money(receipt.discount_minor, receipt.currency_scale)?
            );
            println!(
                "  TOTAL:    {} {}",
                receipt.currency_code,
                format_money(receipt.total_minor, receipt.currency_scale)?
            );
            println!(
                "  Change:   {}",
                format_money(receipt.change_due_minor, receipt.currency_scale)?
            );
        }
        "day" => {
            require_len(&args, 3, "day <database-file> <YYYY-MM-DD-UTC>")?;
            let db = Database::open(&args[1])?;
            let day = db.sales_summary_for_utc_day(&args[2])?;
            let (currency, scale) = db.currency()?;
            println!("Sales summary for {} (UTC)", args[2]);
            println!("  Number of sales: {}", day.sale_count);
            println!(
                "  Subtotal: {} {}",
                currency,
                format_money(day.subtotal_minor, scale)?
            );
            println!(
                "  Discounts: {} {}",
                currency,
                format_money(day.discount_minor, scale)?
            );
            println!(
                "  Net total: {} {}",
                currency,
                format_money(day.net_total_minor, scale)?
            );
        }
        command => {
            return Err(format!("unknown command {command:?}. Run `fastpos help`.").into());
        }
    }
    Ok(())
}

fn parse_i64(value: &str, field: &str) -> Result<i64, Box<dyn Error>> {
    value
        .parse::<i64>()
        .map_err(|_| format!("{field} must be a whole number").into())
}

fn require_len(args: &[String], expected: usize, usage: &str) -> Result<(), Box<dyn Error>> {
    if args.len() != expected {
        return Err(format!("wrong number of arguments. Usage: fastpos {usage}").into());
    }
    Ok(())
}

fn truncate(value: &str, max_chars: usize) -> String {
    let mut chars = value.chars();
    let result: String = chars.by_ref().take(max_chars).collect();
    if chars.next().is_some() {
        format!(
            "{}…",
            result
                .chars()
                .take(max_chars.saturating_sub(1))
                .collect::<String>()
        )
    } else {
        result
    }
}

fn print_help() {
    println!("FastPOS Core 0.1.1 — development CLI\n");
    println!("Commands:");
    println!("  fastpos init <database-file> [country-code]");
    println!("  fastpos set-country <database-file> <country-code>");
    println!("  fastpos set-currency <database-file> <currency-code>");
    println!("  fastpos currency <database-file>");
    println!("  fastpos add-product <database-file> <name> <barcode-or-dash> <price-minor> <stock-milli>");
    println!("  fastpos products <database-file>");
    println!("  fastpos sell <database-file> <product-id> <quantity-milli> <cash-received-minor>");
    println!("  fastpos sale <database-file> <sale-id>");
    println!("  fastpos day <database-file> <YYYY-MM-DD-UTC>\n");
    println!("Storage units:");
    println!("  Money: integer minor units. The default is Tunisia (TN / TND), scale=1000.");
    println!("  Configure a country before adding products/sales, e.g. `init store.sqlite3 US`.");
    println!("  Currency scale comes from ISO 4217 data; USD=100, TND=1000, JPY=1.");
    println!("  Quantity: thousandths of a unit. 1.250 units = 1250.");
    println!("Names containing spaces should be quoted by your shell.");
}
