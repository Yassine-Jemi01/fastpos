use fastpos_core::{
    format_money, CartItem, CheckoutRequest, Database, FastPosError, NewProduct, PaymentInput,
    PaymentMethod,
};
use rusqlite::Connection;
use tempfile::tempdir;

fn product(name: &str, barcode: &str, price: i64, stock: i64) -> NewProduct {
    NewProduct::new(name, Some(barcode.to_string()), price, stock)
}

fn cash_payment(amount: i64) -> PaymentInput {
    PaymentInput {
        method: PaymentMethod::Cash,
        tendered_minor: amount,
    }
}

#[test]
fn sale_is_saved_and_stock_is_reduced_atomically() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("fastpos.sqlite3");
    let mut db = Database::open(path).unwrap();
    let p = db
        .add_product(product("Milk", "61100001", 2_500, 5_000))
        .unwrap();

    let receipt = db
        .checkout(CheckoutRequest {
            items: vec![CartItem {
                product_id: p.id,
                quantity_milli: 2_000,
            }],
            discount_minor: 0,
            payments: vec![cash_payment(6_000)],
        })
        .unwrap();

    assert_eq!(receipt.receipt_number, "FP-00000001");
    assert_eq!(receipt.subtotal_minor, 5_000);
    assert_eq!(receipt.total_minor, 5_000);
    assert_eq!(receipt.change_due_minor, 1_000);
    assert_eq!(receipt.items.len(), 1);
    assert_eq!(receipt.payments[0].applied_minor, 5_000);
    assert_eq!(db.get_product(p.id).unwrap().stock_quantity_milli, 3_000);
    assert_eq!(db.get_sale(receipt.id).unwrap(), receipt);
    assert_eq!(
        format_money(receipt.total_minor, receipt.currency_scale).unwrap(),
        "5.000"
    );
}

#[test]
fn utc_summary_uses_inclusive_start_and_exclusive_end() {
    let mut db = Database::open_in_memory().unwrap();
    let p = db
        .add_product(product("Water", "61100090", 1_000, 3_000))
        .unwrap();
    let receipt = db
        .checkout(CheckoutRequest {
            items: vec![CartItem {
                product_id: p.id,
                quantity_milli: 1_000,
            }],
            discount_minor: 0,
            payments: vec![cash_payment(1_000)],
        })
        .unwrap();

    let after_sale = db
        .sales_summary_for_utc_range(&receipt.sold_at, "9999-12-31T23:59:59.999Z")
        .unwrap();
    assert_eq!(
        after_sale.sale_count, 1,
        "range includes its start boundary"
    );

    let before_sale = db
        .sales_summary_for_utc_range("0001-01-01T00:00:00.000Z", &receipt.sold_at)
        .unwrap();
    assert_eq!(before_sale.sale_count, 0, "range excludes its end boundary");

    let utc_day = db
        .sales_summary_for_utc_day(&receipt.sold_at[..10])
        .unwrap();
    assert_eq!(utc_day.sale_count, 1);
    assert!(db
        .sales_summary_for_utc_range(&receipt.sold_at, &receipt.sold_at)
        .is_err());
}

#[test]
fn partially_applied_legacy_migration_recovers_without_mismatched_country() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("partially-migrated.sqlite3");
    {
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(
            "PRAGMA user_version = 1;
             CREATE TABLE store_settings (
                 id INTEGER PRIMARY KEY CHECK (id = 1),
                 country_code TEXT CHECK (country_code IS NULL OR length(country_code) = 2) DEFAULT 'TN',
                 currency_code TEXT NOT NULL CHECK (length(currency_code) = 3),
                 currency_scale INTEGER NOT NULL CHECK (currency_scale IN (1, 10, 100, 1000, 10000)),
                 updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
             );
             INSERT INTO store_settings (id, country_code, currency_code, currency_scale) VALUES (1, 'TN', 'EUR', 100);",
        ).unwrap();
    }

    let db = Database::open(&path).unwrap();
    assert_eq!(db.country().unwrap(), None);
    assert_eq!(db.currency().unwrap(), ("EUR".to_string(), 100));
}

#[test]
fn fractional_quantity_is_rounded_using_integer_arithmetic() {
    let mut db = Database::open_in_memory().unwrap();
    let p = db
        .add_product(product("Apples", "61100002", 1_999, 2_000))
        .unwrap();
    let receipt = db
        .checkout(CheckoutRequest {
            items: vec![CartItem {
                product_id: p.id,
                quantity_milli: 1_500,
            }],
            discount_minor: 0,
            payments: vec![cash_payment(2_999)],
        })
        .unwrap();
    assert_eq!(receipt.total_minor, 2_999); // 1.999 * 1.500 = 2.9985, rounded half up.
}

#[test]
fn insufficient_stock_does_not_create_a_sale_or_change_stock() {
    let mut db = Database::open_in_memory().unwrap();
    let p = db
        .add_product(product("Bread", "61100003", 750, 1_000))
        .unwrap();
    let result = db.checkout(CheckoutRequest {
        items: vec![CartItem {
            product_id: p.id,
            quantity_milli: 2_000,
        }],
        discount_minor: 0,
        payments: vec![cash_payment(2_000)],
    });
    assert!(matches!(
        result,
        Err(FastPosError::InsufficientStock { .. })
    ));
    assert_eq!(db.get_product(p.id).unwrap().stock_quantity_milli, 1_000);
    assert_eq!(
        db.sales_summary_for_utc_day("2026-01-01")
            .unwrap()
            .sale_count,
        0
    );
}

#[test]
fn invalid_payment_does_not_commit_sale() {
    let mut db = Database::open_in_memory().unwrap();
    let p = db
        .add_product(product("Water", "61100004", 1_250, 2_000))
        .unwrap();
    let result = db.checkout(CheckoutRequest {
        items: vec![CartItem {
            product_id: p.id,
            quantity_milli: 1_000,
        }],
        discount_minor: 0,
        payments: vec![cash_payment(1_000)],
    });
    assert!(matches!(result, Err(FastPosError::PaymentMismatch { .. })));
    assert_eq!(db.get_product(p.id).unwrap().stock_quantity_milli, 2_000);
    assert_eq!(
        db.sales_summary_for_utc_day("2026-01-01")
            .unwrap()
            .sale_count,
        0
    );
}

#[test]
fn duplicate_barcodes_are_rejected() {
    let mut db = Database::open_in_memory().unwrap();
    db.add_product(product("Milk", "61100005", 2_500, 1_000))
        .unwrap();
    let result = db.add_product(product("Other milk", "61100005", 2_700, 1_000));
    assert!(matches!(result, Err(FastPosError::InvalidInput(_))));
}

#[test]
fn discount_and_card_payment_are_recorded() {
    let mut db = Database::open_in_memory().unwrap();
    let p = db
        .add_product(product("Coffee", "61100006", 3_500, 4_000))
        .unwrap();
    let receipt = db
        .checkout(CheckoutRequest {
            items: vec![CartItem {
                product_id: p.id,
                quantity_milli: 2_000,
            }],
            discount_minor: 500,
            payments: vec![PaymentInput {
                method: PaymentMethod::Card,
                tendered_minor: 6_500,
            }],
        })
        .unwrap();
    assert_eq!(receipt.subtotal_minor, 7_000);
    assert_eq!(receipt.discount_minor, 500);
    assert_eq!(receipt.total_minor, 6_500);
    assert_eq!(receipt.change_due_minor, 0);
    assert_eq!(receipt.payments[0].method, "card");
}

#[test]
fn stock_adjustment_is_audited() {
    let mut db = Database::open_in_memory().unwrap();
    let p = db
        .add_product(product("Rice", "61100007", 4_000, 1_000))
        .unwrap();
    let changed = db.adjust_stock(p.id, 2_500, "new delivery").unwrap();
    assert_eq!(changed.stock_quantity_milli, 3_500);
    let result = db.adjust_stock(p.id, -4_000, "count correction");
    assert!(matches!(
        result,
        Err(FastPosError::InsufficientStock { .. })
    ));
    assert_eq!(db.get_product(p.id).unwrap().stock_quantity_milli, 3_500);
}

#[test]
fn currency_change_is_blocked_after_products_exist() {
    let mut db = Database::open_in_memory().unwrap();
    db.add_product(product("Sugar", "61100008", 2_000, 1_000))
        .unwrap();
    assert!(matches!(
        db.set_currency("EUR", 100),
        Err(FastPosError::InvalidInput(_))
    ));
}

#[test]
fn schema_v1_database_is_migrated_without_losing_data() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("legacy.sqlite3");
    {
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(
            "PRAGMA user_version = 1;
             CREATE TABLE store_settings (
                 id INTEGER PRIMARY KEY CHECK (id = 1),
                 currency_code TEXT NOT NULL CHECK (length(currency_code) = 3),
                 currency_scale INTEGER NOT NULL CHECK (currency_scale IN (1, 10, 100, 1000, 10000)),
                 updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
             );
             INSERT INTO store_settings (id, currency_code, currency_scale) VALUES (1, 'EUR', 100);
             CREATE TABLE products (id INTEGER PRIMARY KEY AUTOINCREMENT, name TEXT NOT NULL, barcode TEXT UNIQUE, unit_price_minor INTEGER NOT NULL, stock_quantity_milli INTEGER NOT NULL DEFAULT 0, is_active INTEGER NOT NULL DEFAULT 1, created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')));
             CREATE TABLE sales (id INTEGER PRIMARY KEY AUTOINCREMENT, receipt_number TEXT UNIQUE, currency_code TEXT NOT NULL, currency_scale INTEGER NOT NULL, subtotal_minor INTEGER NOT NULL, discount_minor INTEGER NOT NULL, total_minor INTEGER NOT NULL, change_due_minor INTEGER NOT NULL DEFAULT 0, sold_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')));
             INSERT INTO products (name, barcode, unit_price_minor, stock_quantity_milli) VALUES ('Milk', '61100001', 2500, 5000);",
        ).unwrap();
    }

    let db = Database::open(&path).unwrap();
    assert_eq!(db.country().unwrap(), None);
    assert_eq!(db.currency().unwrap(), ("EUR".to_string(), 100));
    assert_eq!(db.list_products().unwrap().len(), 1);
}
