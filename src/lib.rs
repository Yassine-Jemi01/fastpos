//! FastPOS Core: offline-first sales and inventory logic.
//!
//! Money is stored as integer minor units. The default currency is TND with a
//! scale of 1,000 (so 2.500 TND is represented as 2_500). Product quantities
//! are stored in thousandths of a unit (so 1.250 units is represented as 1_250).
//! No floating-point arithmetic is used for prices or quantities.

use rusqlite::{params, Connection, OptionalExtension, Row, TransactionBehavior};
use std::collections::BTreeMap;
use std::error::Error;
use std::fmt;
use std::path::Path;
use std::time::Duration;

const SCHEMA_VERSION: i64 = 2;
const QUANTITY_SCALE: i64 = 1_000;

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS store_settings (
    id              INTEGER PRIMARY KEY CHECK (id = 1),
    country_code    TEXT CHECK (country_code IS NULL OR length(country_code) = 2),
    currency_code   TEXT NOT NULL CHECK (length(currency_code) = 3),
    currency_scale  INTEGER NOT NULL CHECK (currency_scale IN (1, 10, 100, 1000, 10000)),
    updated_at      TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);

INSERT OR IGNORE INTO store_settings (id, country_code, currency_code, currency_scale)
VALUES (1, 'TN', 'TND', 1000);

CREATE TABLE IF NOT EXISTS products (
    id                    INTEGER PRIMARY KEY AUTOINCREMENT,
    name                  TEXT NOT NULL CHECK (length(trim(name)) > 0),
    barcode               TEXT UNIQUE,
    unit_price_minor      INTEGER NOT NULL CHECK (unit_price_minor > 0),
    stock_quantity_milli  INTEGER NOT NULL DEFAULT 0 CHECK (stock_quantity_milli >= 0),
    is_active             INTEGER NOT NULL DEFAULT 1 CHECK (is_active IN (0, 1)),
    created_at            TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);

CREATE INDEX IF NOT EXISTS idx_products_name ON products(name COLLATE NOCASE);
CREATE INDEX IF NOT EXISTS idx_products_active ON products(is_active);

CREATE TABLE IF NOT EXISTS sales (
    id                INTEGER PRIMARY KEY AUTOINCREMENT,
    receipt_number    TEXT UNIQUE,
    currency_code     TEXT NOT NULL,
    currency_scale    INTEGER NOT NULL CHECK (currency_scale IN (1, 10, 100, 1000, 10000)),
    subtotal_minor    INTEGER NOT NULL CHECK (subtotal_minor >= 0),
    discount_minor    INTEGER NOT NULL CHECK (discount_minor >= 0),
    total_minor       INTEGER NOT NULL CHECK (total_minor > 0),
    change_due_minor  INTEGER NOT NULL DEFAULT 0 CHECK (change_due_minor >= 0),
    sold_at           TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);

CREATE INDEX IF NOT EXISTS idx_sales_sold_at ON sales(sold_at);

CREATE TABLE IF NOT EXISTS sale_items (
    id                INTEGER PRIMARY KEY AUTOINCREMENT,
    sale_id           INTEGER NOT NULL REFERENCES sales(id) ON DELETE RESTRICT,
    product_id        INTEGER NOT NULL REFERENCES products(id) ON DELETE RESTRICT,
    product_name      TEXT NOT NULL,
    barcode           TEXT,
    unit_price_minor  INTEGER NOT NULL CHECK (unit_price_minor > 0),
    quantity_milli    INTEGER NOT NULL CHECK (quantity_milli > 0),
    line_total_minor  INTEGER NOT NULL CHECK (line_total_minor > 0)
);

CREATE INDEX IF NOT EXISTS idx_sale_items_sale_id ON sale_items(sale_id);
CREATE INDEX IF NOT EXISTS idx_sale_items_product_id ON sale_items(product_id);

CREATE TABLE IF NOT EXISTS payments (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    sale_id         INTEGER NOT NULL REFERENCES sales(id) ON DELETE RESTRICT,
    method          TEXT NOT NULL CHECK (method IN ('cash', 'card', 'voucher', 'other')),
    tendered_minor  INTEGER NOT NULL CHECK (tendered_minor > 0),
    applied_minor   INTEGER NOT NULL CHECK (applied_minor > 0)
);

CREATE INDEX IF NOT EXISTS idx_payments_sale_id ON payments(sale_id);

CREATE TABLE IF NOT EXISTS stock_movements (
    id                    INTEGER PRIMARY KEY AUTOINCREMENT,
    product_id            INTEGER NOT NULL REFERENCES products(id) ON DELETE RESTRICT,
    sale_id               INTEGER REFERENCES sales(id) ON DELETE RESTRICT,
    quantity_delta_milli  INTEGER NOT NULL CHECK (quantity_delta_milli != 0),
    reason                TEXT NOT NULL CHECK (length(trim(reason)) > 0),
    created_at            TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);

CREATE INDEX IF NOT EXISTS idx_stock_movements_product_id ON stock_movements(product_id);
"#;

/// Errors returned by the FastPOS Core API.
#[derive(Debug)]
pub enum FastPosError {
    Sql(rusqlite::Error),
    InvalidInput(String),
    NotFound(String),
    InsufficientStock {
        product_id: i64,
        requested_milli: i64,
        available_milli: i64,
    },
    PaymentMismatch {
        expected_minor: i64,
        actual_minor: i64,
    },
}

impl fmt::Display for FastPosError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Sql(error) => write!(f, "database error: {error}"),
            Self::InvalidInput(message) => write!(f, "invalid input: {message}"),
            Self::NotFound(message) => write!(f, "not found: {message}"),
            Self::InsufficientStock {
                product_id,
                requested_milli,
                available_milli,
            } => write!(
                f,
                "insufficient stock for product {product_id}: requested {requested_milli} milli-units, available {available_milli}"
            ),
            Self::PaymentMismatch {
                expected_minor,
                actual_minor,
            } => write!(
                f,
                "payment does not match sale total: expected {expected_minor} minor units, received {actual_minor}"
            ),
        }
    }
}

impl Error for FastPosError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Sql(error) => Some(error),
            _ => None,
        }
    }
}

impl From<rusqlite::Error> for FastPosError {
    fn from(value: rusqlite::Error) -> Self {
        Self::Sql(value)
    }
}

pub type FastPosResult<T> = std::result::Result<T, FastPosError>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Product {
    pub id: i64,
    pub name: String,
    pub barcode: Option<String>,
    pub unit_price_minor: i64,
    pub stock_quantity_milli: i64,
    pub is_active: bool,
    pub created_at: String,
}

#[derive(Debug, Clone)]
pub struct NewProduct {
    pub name: String,
    pub barcode: Option<String>,
    pub unit_price_minor: i64,
    pub initial_stock_milli: i64,
}

impl NewProduct {
    pub fn new(
        name: impl Into<String>,
        barcode: Option<String>,
        unit_price_minor: i64,
        initial_stock_milli: i64,
    ) -> Self {
        Self {
            name: name.into(),
            barcode,
            unit_price_minor,
            initial_stock_milli,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaymentMethod {
    Cash,
    Card,
    Voucher,
    Other,
}

impl PaymentMethod {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Cash => "cash",
            Self::Card => "card",
            Self::Voucher => "voucher",
            Self::Other => "other",
        }
    }
}

#[derive(Debug, Clone)]
pub struct CartItem {
    pub product_id: i64,
    /// Quantity in thousandths: 1_000 = 1 item/unit, 1_250 = 1.250 units.
    pub quantity_milli: i64,
}

#[derive(Debug, Clone)]
pub struct PaymentInput {
    pub method: PaymentMethod,
    /// Money handed/charged, represented in the store currency's minor units.
    pub tendered_minor: i64,
}

#[derive(Debug, Clone)]
pub struct CheckoutRequest {
    pub items: Vec<CartItem>,
    /// Fixed amount discount in minor units. Percentage discounts can be added
    /// by the UI/service layer and passed here as a calculated amount.
    pub discount_minor: i64,
    pub payments: Vec<PaymentInput>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SaleItem {
    pub product_id: i64,
    pub product_name: String,
    pub barcode: Option<String>,
    pub unit_price_minor: i64,
    pub quantity_milli: i64,
    pub line_total_minor: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaymentRecord {
    pub method: String,
    pub tendered_minor: i64,
    pub applied_minor: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Receipt {
    pub id: i64,
    pub receipt_number: String,
    pub sold_at: String,
    pub currency_code: String,
    pub currency_scale: i64,
    pub subtotal_minor: i64,
    pub discount_minor: i64,
    pub total_minor: i64,
    pub change_due_minor: i64,
    pub items: Vec<SaleItem>,
    pub payments: Vec<PaymentRecord>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SalesSummary {
    /// Inclusive beginning of the summary period, in canonical UTC format.
    pub period_start_utc: String,
    /// Exclusive end of the summary period, in canonical UTC format.
    pub period_end_utc: String,
    pub sale_count: i64,
    pub subtotal_minor: i64,
    pub discount_minor: i64,
    pub net_total_minor: i64,
}

/// A single SQLite connection and the FastPOS schema.
pub struct Database {
    conn: Connection,
}

impl Database {
    /// Open (or create) a database file and initialize the schema.
    pub fn open(path: impl AsRef<Path>) -> FastPosResult<Self> {
        let conn = Connection::open(path)?;
        Self::initialize(conn)
    }

    /// Create an in-memory database, useful for tests and examples.
    pub fn open_in_memory() -> FastPosResult<Self> {
        let conn = Connection::open_in_memory()?;
        Self::initialize(conn)
    }

    fn initialize(conn: Connection) -> FastPosResult<Self> {
        let mut conn = conn;
        conn.busy_timeout(Duration::from_secs(5))?;
        conn.execute_batch("PRAGMA foreign_keys = ON; PRAGMA synchronous = FULL;")?;
        // WAL improves recovery and concurrent read behavior for file databases.
        // SQLite may report "memory" instead for an in-memory database.
        let _: String = conn.query_row("PRAGMA journal_mode = WAL", [], |row| row.get(0))?;

        // Serialize initialization and make schema changes plus user_version one
        // atomic operation. A crash cannot leave a half-applied migration.
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let version: i64 = tx.query_row("PRAGMA user_version", [], |row| row.get(0))?;
        if version > SCHEMA_VERSION {
            return Err(FastPosError::InvalidInput(format!(
                "database schema version {version} is newer than this FastPOS Core supports ({SCHEMA_VERSION})"
            )));
        }

        if version == 1 {
            // Recover if an older pre-release added this column but crashed
            // before updating user_version.
            let has_country_code: bool = tx.query_row(
                "SELECT EXISTS (SELECT 1 FROM pragma_table_info('store_settings') WHERE name = 'country_code')",
                [],
                |row| row.get(0),
            )?;
            if !has_country_code {
                // NULL preserves legacy currencies without falsely assigning TN.
                tx.execute_batch(
                    "ALTER TABLE store_settings ADD COLUMN country_code TEXT CHECK (country_code IS NULL OR length(country_code) = 2);",
                )?;
            }
        }

        tx.execute_batch(SCHEMA)?;
        // Repair any database touched by the old migration, including ones
        // already marked v2, which assigned TN despite a non-TND currency.
        tx.execute(
            "UPDATE store_settings SET country_code = NULL WHERE country_code = 'TN' AND currency_code <> 'TND'",
            [],
        )?;
        tx.pragma_update(None, "user_version", SCHEMA_VERSION)?;
        tx.commit()?;
        Ok(Self { conn })
    }

    /// Return the configured ISO 4217 currency code and its minor-unit scale.
    /// For TND the default scale is 1,000 (one dinar = 1,000 minor units).
    pub fn currency(&self) -> FastPosResult<(String, i64)> {
        Ok(self.conn.query_row(
            "SELECT currency_code, currency_scale FROM store_settings WHERE id = 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?)
    }

    /// Return the configured ISO 3166-1 alpha-2 country code, if selected.
    pub fn country(&self) -> FastPosResult<Option<String>> {
        Ok(self.conn.query_row(
            "SELECT country_code FROM store_settings WHERE id = 1",
            [],
            |row| row.get(0),
        )?)
    }

    /// Select a country's primary ISO 4217 currency from its ISO alpha-2 code,
    /// e.g. `TN` for Tunisia or `US` for the United States.
    /// Configure this before adding products or sales so existing monetary
    /// values can never be reinterpreted as a different currency.
    pub fn set_country(&mut self, country_code: &str) -> FastPosResult<()> {
        let country_code = country_code.trim().to_ascii_uppercase();
        if country_code.len() != 2 || !country_code.bytes().all(|byte| byte.is_ascii_alphabetic()) {
            return Err(FastPosError::InvalidInput(
                "country must be a two-letter ISO 3166-1 code, such as TN, US, or FR".into(),
            ));
        }
        let country = country_code.parse::<iso_currency::Country>().map_err(|_| {
            FastPosError::InvalidInput(format!("unknown ISO 3166-1 country code {country_code:?}"))
        })?;
        // This conversion selects the country's regular currency and filters
        // superseded, fund, and special-purpose currency codes.
        let currency = iso_currency::Currency::from(country);
        let exponent = currency.exponent().ok_or_else(|| {
            FastPosError::InvalidInput(format!(
                "currency {} has no supported minor-unit exponent",
                currency.code()
            ))
        })?;
        let scale = scale_for_currency_exponent(exponent)?;
        self.set_currency_internal(currency.code(), scale, Some(&country_code))
    }

    /// Select a currency by ISO 4217 code, e.g. `TND`, `USD`, `EUR`, or `JPY`.
    /// This clears the country selection because a currency code alone does not
    /// uniquely identify a store's country.
    pub fn set_currency_code(&mut self, code: &str) -> FastPosResult<()> {
        let code = code.trim().to_ascii_uppercase();
        let currency = iso_currency::Currency::from_code(&code).ok_or_else(|| {
            FastPosError::InvalidInput(format!("unknown ISO 4217 currency code {code:?}"))
        })?;
        let exponent = currency.exponent().ok_or_else(|| {
            FastPosError::InvalidInput(format!(
                "currency {} has no supported minor-unit exponent",
                currency.code()
            ))
        })?;
        let scale = scale_for_currency_exponent(exponent)?;
        self.set_currency_internal(currency.code(), scale, None)
    }

    /// Configure an ISO 4217 currency before creating products or sales.
    /// The scale must match the ISO minor-unit exponent for that currency.
    pub fn set_currency(&mut self, code: &str, scale: i64) -> FastPosResult<()> {
        let normalized = code.trim().to_ascii_uppercase();
        let currency = iso_currency::Currency::from_code(&normalized).ok_or_else(|| {
            FastPosError::InvalidInput(format!("unknown ISO 4217 currency code {normalized:?}"))
        })?;
        let exponent = currency.exponent().ok_or_else(|| {
            FastPosError::InvalidInput(format!(
                "currency {} has no supported minor-unit exponent",
                currency.code()
            ))
        })?;
        let expected_scale = scale_for_currency_exponent(exponent)?;
        if scale != expected_scale {
            return Err(FastPosError::InvalidInput(format!(
                "currency {} requires scale {expected_scale}, not {scale}",
                currency.code()
            )));
        }
        self.set_currency_internal(currency.code(), scale, None)
    }

    fn set_currency_internal(
        &mut self,
        code: &str,
        scale: i64,
        country_code: Option<&str>,
    ) -> FastPosResult<()> {
        let code = code.trim().to_ascii_uppercase();
        if code.len() != 3 || !code.bytes().all(|byte| byte.is_ascii_alphabetic()) {
            return Err(FastPosError::InvalidInput(
                "currency code must be three ASCII letters, such as TND or EUR".into(),
            ));
        }
        validate_currency_scale(scale)?;
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let count: i64 = tx.query_row(
            "SELECT (SELECT COUNT(*) FROM products) + (SELECT COUNT(*) FROM sales)",
            [],
            |row| row.get(0),
        )?;
        if count != 0 {
            return Err(FastPosError::InvalidInput(
                "country/currency cannot be changed after products or sales exist".into(),
            ));
        }
        tx.execute(
            "UPDATE store_settings SET country_code = ?1, currency_code = ?2, currency_scale = ?3, updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now') WHERE id = 1",
            params![country_code, code, scale],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Add a product and record its opening stock.
    pub fn add_product(&mut self, input: NewProduct) -> FastPosResult<Product> {
        let name = input.name.trim().to_string();
        if name.is_empty() {
            return Err(FastPosError::InvalidInput(
                "product name cannot be empty".into(),
            ));
        }
        if input.unit_price_minor <= 0 {
            return Err(FastPosError::InvalidInput(
                "product price must be greater than zero".into(),
            ));
        }
        if input.initial_stock_milli < 0 {
            return Err(FastPosError::InvalidInput(
                "initial stock cannot be negative".into(),
            ));
        }
        let barcode = input.barcode.and_then(|value| {
            let trimmed = value.trim().to_string();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed)
            }
        });

        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        if let Some(value) = barcode.as_deref() {
            let exists: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM products WHERE barcode = ?1)",
                [value],
                |row| row.get(0),
            )?;
            if exists {
                return Err(FastPosError::InvalidInput(format!(
                    "barcode {value:?} is already assigned to a product"
                )));
            }
        }
        tx.execute(
            "INSERT INTO products (name, barcode, unit_price_minor, stock_quantity_milli) VALUES (?1, ?2, ?3, ?4)",
            params![name, barcode, input.unit_price_minor, input.initial_stock_milli],
        )?;
        let product_id = tx.last_insert_rowid();
        if input.initial_stock_milli > 0 {
            tx.execute(
                "INSERT INTO stock_movements (product_id, sale_id, quantity_delta_milli, reason) VALUES (?1, NULL, ?2, 'opening stock')",
                params![product_id, input.initial_stock_milli],
            )?;
        }
        tx.commit()?;
        self.get_product(product_id)
    }

    pub fn get_product(&self, product_id: i64) -> FastPosResult<Product> {
        self.conn
            .query_row(
                "SELECT id, name, barcode, unit_price_minor, stock_quantity_milli, is_active, created_at FROM products WHERE id = ?1",
                [product_id],
                row_to_product,
            )
            .optional()?
            .ok_or_else(|| FastPosError::NotFound(format!("product {product_id}")))
    }

    pub fn find_by_barcode(&self, barcode: &str) -> FastPosResult<Option<Product>> {
        Ok(self.conn.query_row(
            "SELECT id, name, barcode, unit_price_minor, stock_quantity_milli, is_active, created_at FROM products WHERE barcode = ?1 AND is_active = 1",
            [barcode.trim()],
            row_to_product,
        ).optional()?)
    }

    pub fn list_products(&self) -> FastPosResult<Vec<Product>> {
        let mut statement = self.conn.prepare(
            "SELECT id, name, barcode, unit_price_minor, stock_quantity_milli, is_active, created_at FROM products ORDER BY is_active DESC, name COLLATE NOCASE ASC, id ASC",
        )?;
        let rows = statement.query_map([], row_to_product)?;
        let mut products = Vec::new();
        for row in rows {
            products.push(row?);
        }
        Ok(products)
    }

    pub fn set_product_active(&mut self, product_id: i64, active: bool) -> FastPosResult<()> {
        let changed = self.conn.execute(
            "UPDATE products SET is_active = ?1 WHERE id = ?2",
            params![active, product_id],
        )?;
        if changed == 0 {
            return Err(FastPosError::NotFound(format!("product {product_id}")));
        }
        Ok(())
    }

    /// Adjust stock by a non-zero signed quantity in thousandths of a unit.
    pub fn adjust_stock(
        &mut self,
        product_id: i64,
        delta_milli: i64,
        reason: &str,
    ) -> FastPosResult<Product> {
        if delta_milli == 0 {
            return Err(FastPosError::InvalidInput(
                "stock adjustment cannot be zero".into(),
            ));
        }
        let reason = reason.trim();
        if reason.is_empty() {
            return Err(FastPosError::InvalidInput(
                "stock adjustment reason cannot be empty".into(),
            ));
        }
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let current: i64 = tx
            .query_row(
                "SELECT stock_quantity_milli FROM products WHERE id = ?1",
                [product_id],
                |row| row.get(0),
            )
            .optional()?
            .ok_or_else(|| FastPosError::NotFound(format!("product {product_id}")))?;
        let next = current
            .checked_add(delta_milli)
            .ok_or_else(|| FastPosError::InvalidInput("stock quantity overflow".into()))?;
        if next < 0 {
            return Err(FastPosError::InsufficientStock {
                product_id,
                requested_milli: delta_milli.checked_neg().unwrap_or(i64::MAX),
                available_milli: current,
            });
        }
        tx.execute(
            "UPDATE products SET stock_quantity_milli = ?1 WHERE id = ?2",
            params![next, product_id],
        )?;
        tx.execute(
            "INSERT INTO stock_movements (product_id, sale_id, quantity_delta_milli, reason) VALUES (?1, NULL, ?2, ?3)",
            params![product_id, delta_milli, reason],
        )?;
        tx.commit()?;
        self.get_product(product_id)
    }

    /// Complete a sale atomically: validate inventory and payment, create the
    /// receipt, record payment and stock movements, then reduce stock.
    pub fn checkout(&mut self, request: CheckoutRequest) -> FastPosResult<Receipt> {
        if request.items.is_empty() {
            return Err(FastPosError::InvalidInput(
                "a sale must contain at least one item".into(),
            ));
        }
        if request.discount_minor < 0 {
            return Err(FastPosError::InvalidInput(
                "discount cannot be negative".into(),
            ));
        }

        let mut combined: BTreeMap<i64, i64> = BTreeMap::new();
        for item in &request.items {
            if item.product_id <= 0 {
                return Err(FastPosError::InvalidInput(
                    "product ID must be positive".into(),
                ));
            }
            if item.quantity_milli <= 0 {
                return Err(FastPosError::InvalidInput(
                    "item quantity must be positive".into(),
                ));
            }
            let previous = combined.get(&item.product_id).copied().unwrap_or(0);
            let quantity = previous.checked_add(item.quantity_milli).ok_or_else(|| {
                FastPosError::InvalidInput("combined item quantity overflow".into())
            })?;
            combined.insert(item.product_id, quantity);
        }

        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let (currency_code, currency_scale): (String, i64) = tx.query_row(
            "SELECT currency_code, currency_scale FROM store_settings WHERE id = 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;

        let mut items = Vec::with_capacity(combined.len());
        let mut subtotal_minor: i64 = 0;
        for (product_id, quantity_milli) in &combined {
            let product = tx
                .query_row(
                    "SELECT id, name, barcode, unit_price_minor, stock_quantity_milli, is_active, created_at FROM products WHERE id = ?1",
                    [product_id],
                    row_to_product,
                )
                .optional()?
                .ok_or_else(|| FastPosError::NotFound(format!("product {product_id}")))?;
            if !product.is_active {
                return Err(FastPosError::InvalidInput(format!(
                    "product {} is inactive and cannot be sold",
                    product.id
                )));
            }
            if product.stock_quantity_milli < *quantity_milli {
                return Err(FastPosError::InsufficientStock {
                    product_id: product.id,
                    requested_milli: *quantity_milli,
                    available_milli: product.stock_quantity_milli,
                });
            }
            let line_total = calculate_line_total(product.unit_price_minor, *quantity_milli)?;
            if line_total <= 0 {
                return Err(FastPosError::InvalidInput(format!(
                    "quantity for product {} is too small to produce a non-zero line total",
                    product.id
                )));
            }
            subtotal_minor = subtotal_minor
                .checked_add(line_total)
                .ok_or_else(|| FastPosError::InvalidInput("sale subtotal overflow".into()))?;
            items.push(SaleItem {
                product_id: product.id,
                product_name: product.name,
                barcode: product.barcode,
                unit_price_minor: product.unit_price_minor,
                quantity_milli: *quantity_milli,
                line_total_minor: line_total,
            });
        }

        // A zero-value sale is deliberately rejected in this first version.
        if request.discount_minor >= subtotal_minor {
            return Err(FastPosError::InvalidInput(
                "discount must be smaller than the sale subtotal".into(),
            ));
        }
        let total_minor = subtotal_minor - request.discount_minor;
        if request.payments.is_empty() {
            return Err(FastPosError::InvalidInput(
                "at least one payment is required".into(),
            ));
        }
        let mut tendered_total: i64 = 0;
        for payment in &request.payments {
            if payment.tendered_minor <= 0 {
                return Err(FastPosError::InvalidInput(
                    "payment amount must be greater than zero".into(),
                ));
            }
            tendered_total = tendered_total
                .checked_add(payment.tendered_minor)
                .ok_or_else(|| FastPosError::InvalidInput("payment total overflow".into()))?;
        }

        let (change_due_minor, payment_records) =
            if request.payments.len() == 1 && request.payments[0].method == PaymentMethod::Cash {
                let tendered = request.payments[0].tendered_minor;
                if tendered < total_minor {
                    return Err(FastPosError::PaymentMismatch {
                        expected_minor: total_minor,
                        actual_minor: tendered,
                    });
                }
                (
                    tendered - total_minor,
                    vec![PaymentRecord {
                        method: PaymentMethod::Cash.as_str().to_string(),
                        tendered_minor: tendered,
                        applied_minor: total_minor,
                    }],
                )
            } else {
                if tendered_total != total_minor {
                    return Err(FastPosError::PaymentMismatch {
                        expected_minor: total_minor,
                        actual_minor: tendered_total,
                    });
                }
                (
                    0,
                    request
                        .payments
                        .iter()
                        .map(|payment| PaymentRecord {
                            method: payment.method.as_str().to_string(),
                            tendered_minor: payment.tendered_minor,
                            applied_minor: payment.tendered_minor,
                        })
                        .collect(),
                )
            };

        tx.execute(
            "INSERT INTO sales (receipt_number, currency_code, currency_scale, subtotal_minor, discount_minor, total_minor, change_due_minor) VALUES (NULL, ?1, ?2, ?3, ?4, ?5, ?6)",
            params![currency_code, currency_scale, subtotal_minor, request.discount_minor, total_minor, change_due_minor],
        )?;
        let sale_id = tx.last_insert_rowid();
        let receipt_number = format!("FP-{sale_id:08}");
        tx.execute(
            "UPDATE sales SET receipt_number = ?1 WHERE id = ?2",
            params![receipt_number, sale_id],
        )?;

        for item in &items {
            tx.execute(
                "INSERT INTO sale_items (sale_id, product_id, product_name, barcode, unit_price_minor, quantity_milli, line_total_minor) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![sale_id, item.product_id, item.product_name, item.barcode, item.unit_price_minor, item.quantity_milli, item.line_total_minor],
            )?;
            let changed = tx.execute(
                "UPDATE products SET stock_quantity_milli = stock_quantity_milli - ?1 WHERE id = ?2 AND is_active = 1 AND stock_quantity_milli >= ?1",
                params![item.quantity_milli, item.product_id],
            )?;
            if changed != 1 {
                let available: i64 = tx.query_row(
                    "SELECT stock_quantity_milli FROM products WHERE id = ?1",
                    [item.product_id],
                    |row| row.get(0),
                )?;
                return Err(FastPosError::InsufficientStock {
                    product_id: item.product_id,
                    requested_milli: item.quantity_milli,
                    available_milli: available,
                });
            }
            tx.execute(
                "INSERT INTO stock_movements (product_id, sale_id, quantity_delta_milli, reason) VALUES (?1, ?2, ?3, 'sale')",
                params![item.product_id, sale_id, -item.quantity_milli],
            )?;
        }

        for payment in &payment_records {
            tx.execute(
                "INSERT INTO payments (sale_id, method, tendered_minor, applied_minor) VALUES (?1, ?2, ?3, ?4)",
                params![sale_id, payment.method, payment.tendered_minor, payment.applied_minor],
            )?;
        }
        // Read the receipt before commit so a post-commit query failure cannot
        // surface as a failed sale and cause callers to retry a sale already saved.
        let receipt = read_sale(&tx, sale_id)?;
        tx.commit()?;
        Ok(receipt)
    }

    pub fn get_sale(&self, sale_id: i64) -> FastPosResult<Receipt> {
        read_sale(&self.conn, sale_id)
    }

    /// Return totals for a UTC calendar day (`YYYY-MM-DD`).
    pub fn sales_summary_for_utc_day(&self, day: &str) -> FastPosResult<SalesSummary> {
        let next_day = next_utc_day(day)?;
        let start = format!("{day}T00:00:00.000Z");
        let end = format!("{next_day}T00:00:00.000Z");
        self.sales_summary_for_utc_range(&start, &end)
    }

    /// Summarize sales in `[start_utc, end_utc)` using canonical UTC timestamps
    /// (`YYYY-MM-DDTHH:MM:SS.sssZ`). The UI can convert local business-day
    /// boundaries to UTC and call this method, including for DST transition days.
    pub fn sales_summary_for_utc_range(
        &self,
        start_utc: &str,
        end_utc: &str,
    ) -> FastPosResult<SalesSummary> {
        validate_utc_timestamp(start_utc)?;
        validate_utc_timestamp(end_utc)?;
        if start_utc >= end_utc {
            return Err(FastPosError::InvalidInput(
                "summary range start must be earlier than its exclusive end".into(),
            ));
        }
        let (sale_count, subtotal_minor, discount_minor, net_total_minor) = self.conn.query_row(
            "SELECT COUNT(*), COALESCE(SUM(subtotal_minor), 0), COALESCE(SUM(discount_minor), 0), COALESCE(SUM(total_minor), 0) FROM sales WHERE sold_at >= ?1 AND sold_at < ?2",
            params![start_utc, end_utc],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )?;
        Ok(SalesSummary {
            period_start_utc: start_utc.to_string(),
            period_end_utc: end_utc.to_string(),
            sale_count,
            subtotal_minor,
            discount_minor,
            net_total_minor,
        })
    }
}

fn read_sale(conn: &Connection, sale_id: i64) -> FastPosResult<Receipt> {
    let base = conn
        .query_row(
            "SELECT id, receipt_number, sold_at, currency_code, currency_scale, subtotal_minor, discount_minor, total_minor, change_due_minor FROM sales WHERE id = ?1",
            [sale_id],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, i64>(5)?,
                    row.get::<_, i64>(6)?,
                    row.get::<_, i64>(7)?,
                    row.get::<_, i64>(8)?,
                ))
            },
        )
        .optional()?
        .ok_or_else(|| FastPosError::NotFound(format!("sale {sale_id}")))?;

    let (
        id,
        receipt_number,
        sold_at,
        currency_code,
        currency_scale,
        subtotal_minor,
        discount_minor,
        total_minor,
        change_due_minor,
    ) = base;
    let mut item_statement = conn.prepare(
        "SELECT product_id, product_name, barcode, unit_price_minor, quantity_milli, line_total_minor FROM sale_items WHERE sale_id = ?1 ORDER BY id ASC",
    )?;
    let item_rows = item_statement.query_map([sale_id], |row| {
        Ok(SaleItem {
            product_id: row.get(0)?,
            product_name: row.get(1)?,
            barcode: row.get(2)?,
            unit_price_minor: row.get(3)?,
            quantity_milli: row.get(4)?,
            line_total_minor: row.get(5)?,
        })
    })?;
    let mut items = Vec::new();
    for row in item_rows {
        items.push(row?);
    }
    drop(item_statement);

    let mut payment_statement = conn.prepare(
        "SELECT method, tendered_minor, applied_minor FROM payments WHERE sale_id = ?1 ORDER BY id ASC",
    )?;
    let payment_rows = payment_statement.query_map([sale_id], |row| {
        Ok(PaymentRecord {
            method: row.get(0)?,
            tendered_minor: row.get(1)?,
            applied_minor: row.get(2)?,
        })
    })?;
    let mut payments = Vec::new();
    for row in payment_rows {
        payments.push(row?);
    }

    let receipt_number = receipt_number.ok_or_else(|| {
        FastPosError::InvalidInput(format!("sale {sale_id} has no receipt number"))
    })?;
    Ok(Receipt {
        id,
        receipt_number,
        sold_at,
        currency_code,
        currency_scale,
        subtotal_minor,
        discount_minor,
        total_minor,
        change_due_minor,
        items,
        payments,
    })
}

fn row_to_product(row: &Row<'_>) -> rusqlite::Result<Product> {
    Ok(Product {
        id: row.get(0)?,
        name: row.get(1)?,
        barcode: row.get(2)?,
        unit_price_minor: row.get(3)?,
        stock_quantity_milli: row.get(4)?,
        is_active: row.get::<_, i64>(5)? != 0,
        created_at: row.get(6)?,
    })
}

fn calculate_line_total(unit_price_minor: i64, quantity_milli: i64) -> FastPosResult<i64> {
    let product = (unit_price_minor as i128) * (quantity_milli as i128);
    let rounded = (product + (QUANTITY_SCALE as i128 / 2)) / QUANTITY_SCALE as i128;
    if rounded <= 0 || rounded > i64::MAX as i128 {
        return Err(FastPosError::InvalidInput(
            "line total is outside the supported range".into(),
        ));
    }
    Ok(rounded as i64)
}

fn scale_for_currency_exponent(exponent: u16) -> FastPosResult<i64> {
    if exponent > 4 {
        return Err(FastPosError::InvalidInput(format!(
            "currency decimal exponent {exponent} is not supported (expected 0 through 4)"
        )));
    }
    Ok(10_i64.pow(exponent as u32))
}

fn validate_currency_scale(scale: i64) -> FastPosResult<()> {
    if ![1, 10, 100, 1_000, 10_000].contains(&scale) {
        return Err(FastPosError::InvalidInput(
            "currency scale must be 1, 10, 100, 1000, or 10000".into(),
        ));
    }
    Ok(())
}

fn validate_date_shape(day: &str) -> FastPosResult<()> {
    let bytes = day.as_bytes();
    if bytes.len() != 10
        || bytes[4] != b'-'
        || bytes[7] != b'-'
        || !bytes
            .iter()
            .enumerate()
            .all(|(index, byte)| index == 4 || index == 7 || byte.is_ascii_digit())
    {
        return Err(FastPosError::InvalidInput(
            "date must use YYYY-MM-DD format (UTC)".into(),
        ));
    }
    let year = day[0..4].parse::<u32>().unwrap_or(0);
    let month = day[5..7].parse::<u32>().unwrap_or(0);
    let date = day[8..10].parse::<u32>().unwrap_or(0);
    let days_in_month = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if year % 4 == 0 && (year % 100 != 0 || year % 400 == 0) => 29,
        2 => 28,
        _ => 0,
    };
    if year == 0 || date == 0 || date > days_in_month {
        return Err(FastPosError::InvalidInput(
            "date must be a real Gregorian date in YYYY-MM-DD format".into(),
        ));
    }
    Ok(())
}

fn next_utc_day(day: &str) -> FastPosResult<String> {
    validate_date_shape(day)?;
    let mut year = day[0..4].parse::<u32>().unwrap();
    let mut month = day[5..7].parse::<u32>().unwrap();
    let mut date = day[8..10].parse::<u32>().unwrap();
    let last_day = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if year % 4 == 0 && (year % 100 != 0 || year % 400 == 0) => 29,
        2 => 28,
        _ => unreachable!("validate_date_shape rejects invalid months"),
    };
    if date < last_day {
        date += 1;
    } else if month < 12 {
        date = 1;
        month += 1;
    } else {
        if year == 9999 {
            return Err(FastPosError::InvalidInput(
                "date is too late to calculate the next day".into(),
            ));
        }
        date = 1;
        month = 1;
        year += 1;
    }
    Ok(format!("{year:04}-{month:02}-{date:02}"))
}

fn validate_utc_timestamp(timestamp: &str) -> FastPosResult<()> {
    let bytes = timestamp.as_bytes();
    if bytes.len() != 24
        || bytes[10] != b'T'
        || bytes[13] != b':'
        || bytes[16] != b':'
        || bytes[19] != b'.'
        || bytes[23] != b'Z'
        || !bytes.iter().enumerate().all(|(index, byte)| {
            matches!(index, 4 | 7 | 10 | 13 | 16 | 19 | 23) || byte.is_ascii_digit()
        })
    {
        return Err(FastPosError::InvalidInput(
            "timestamp must use canonical UTC format YYYY-MM-DDTHH:MM:SS.sssZ".into(),
        ));
    }
    validate_date_shape(&timestamp[0..10])?;
    let hour = timestamp[11..13].parse::<u32>().unwrap_or(99);
    let minute = timestamp[14..16].parse::<u32>().unwrap_or(99);
    let second = timestamp[17..19].parse::<u32>().unwrap_or(99);
    if hour > 23 || minute > 59 || second > 59 {
        return Err(FastPosError::InvalidInput(
            "timestamp contains an invalid time".into(),
        ));
    }
    Ok(())
}

/// Format integer money without floating-point arithmetic.
pub fn format_money(amount_minor: i64, scale: i64) -> FastPosResult<String> {
    validate_currency_scale(scale)?;
    let negative = amount_minor < 0;
    let absolute = (amount_minor as i128).abs();
    let scale = scale as i128;
    let major = absolute / scale;
    let fraction = absolute % scale;
    let prefix = if negative { "-" } else { "" };
    let decimal_places = (scale as u64).ilog10() as usize;
    if decimal_places == 0 {
        Ok(format!("{prefix}{major}"))
    } else {
        Ok(format!(
            "{prefix}{major}.{fraction:0width$}",
            width = decimal_places
        ))
    }
}

/// Format a quantity stored in thousandths of a unit (e.g. 1_250 => "1.250").
pub fn format_quantity(quantity_milli: i64) -> String {
    let negative = quantity_milli < 0;
    let absolute = (quantity_milli as i128).abs();
    let major = absolute / QUANTITY_SCALE as i128;
    let fraction = absolute % QUANTITY_SCALE as i128;
    format!(
        "{}{}.{:03}",
        if negative { "-" } else { "" },
        major,
        fraction
    )
}

#[cfg(test)]
mod unit_tests {
    use super::*;

    #[test]
    fn money_formatting_works_for_tnd_and_two_decimal_currencies() {
        assert_eq!(format_money(2_500, 1_000).unwrap(), "2.500");
        assert_eq!(format_money(-75, 100).unwrap(), "-0.75");
        assert_eq!(format_money(42, 1).unwrap(), "42");
    }

    #[test]
    fn country_selects_currency_and_minor_unit_scale() {
        let mut db = Database::open_in_memory().unwrap();

        db.set_country("tn").unwrap();
        assert_eq!(db.country().unwrap().as_deref(), Some("TN"));
        assert_eq!(db.currency().unwrap(), ("TND".to_string(), 1_000));

        db.set_country("US").unwrap();
        assert_eq!(db.country().unwrap().as_deref(), Some("US"));
        assert_eq!(db.currency().unwrap(), ("USD".to_string(), 100));

        db.set_currency_code("JPY").unwrap();
        assert_eq!(db.country().unwrap(), None);
        assert_eq!(db.currency().unwrap(), ("JPY".to_string(), 1));
    }

    #[test]
    fn unsupported_country_code_is_rejected() {
        let mut db = Database::open_in_memory().unwrap();
        assert!(matches!(
            db.set_country("ZZ"),
            Err(FastPosError::InvalidInput(_))
        ));
        assert!(matches!(
            db.set_country("TUN"),
            Err(FastPosError::InvalidInput(_))
        ));
    }

    #[test]
    fn manual_currency_scale_must_match_iso_metadata() {
        let mut db = Database::open_in_memory().unwrap();
        assert!(matches!(
            db.set_currency("ABC", 100),
            Err(FastPosError::InvalidInput(_))
        ));
        assert!(matches!(
            db.set_currency("TND", 100),
            Err(FastPosError::InvalidInput(_))
        ));
        db.set_currency("USD", 100).unwrap();
        assert_eq!(db.currency().unwrap(), ("USD".to_string(), 100));
    }

    #[test]
    fn utc_date_validation_handles_calendar_boundaries() {
        assert_eq!(next_utc_day("2024-02-28").unwrap(), "2024-02-29");
        assert_eq!(next_utc_day("2026-02-28").unwrap(), "2026-03-01");
        assert_eq!(next_utc_day("2026-12-31").unwrap(), "2027-01-01");
        assert!(next_utc_day("2026-02-29").is_err());
        assert!(validate_utc_timestamp("2026-10-23T09:15:30.123Z").is_ok());
        assert!(validate_utc_timestamp("2026-10-23T24:00:00.000Z").is_err());
        assert!(validate_utc_timestamp("2026/10/23T09:15:30.123Z").is_err());
    }

    #[test]
    fn quantity_formatting_handles_fractional_units() {
        assert_eq!(format_quantity(1_250), "1.250");
        assert_eq!(format_quantity(-500), "-0.500");
    }
}
