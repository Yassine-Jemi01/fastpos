# Changelog

## 0.1.2

- Fix canonical UTC timestamp validation to allow the required hyphens in `YYYY-MM-DD` dates.
- Add a regression test proving a valid UTC timestamp is accepted and an invalid date separator is rejected.

## 0.1.1

- Replace date substring filtering with an indexed UTC half-open time range for sales summaries.
- Add `sales_summary_for_utc_range` so the future UI can report local business days by converting their boundaries to UTC.
- Validate calendar dates and canonical UTC timestamps used by sales summaries.
- Run schema initialization, migration, and `user_version` update in a single SQLite transaction.
- Preserve legacy currency settings during v1 migration, recover if the earlier non-atomic migration left the country column behind, and repair inconsistent legacy `TN`/non-`TND` settings.
- Validate ISO currency codes and require the scale to match the currency's minor-unit exponent.
- Read the receipt within the checkout transaction before committing, avoiding a post-commit read failure being reported as a failed sale.
- Add regression tests for summary boundaries, date validation, manual currency-scale validation, and migration recovery.

## 0.1.0

- Initial offline-first Rust + SQLite sales and inventory core.
