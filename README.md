# FastPOS OS

**FastPOS OS is an experimental, Debian-based Linux distribution for point-of-sale (POS) and cashier systems.**

The project is in the **alpha stage**. It is being developed and tested, and it is not ready for use as the primary system in a real shop.

- **Current release:** [FastPOS OS 0.2.4-alpha](https://github.com/Yassine-Jemi01/fastpos/releases/tag/0.2.4-alpha)
- **Target architecture:** x86_64 / AMD64
- **License:** GNU GPL v3.0 only

## What is FastPOS OS?

FastPOS OS is designed to provide a lightweight Linux environment for cashier workflows. The current alpha includes a graphical cashier application and a product catalogue. The project also includes a Rust-based sales and inventory core that uses SQLite for local storage.

This is an early testing release. Features and behavior can change between releases.

## Current alpha

### Available for testing

- Bootable Debian-based AMD64 system image.
- Graphical cashier application.
- Product catalogue with manual product entry.
- Barcode-oriented product lookup workflow.
- Rust and SQLite core for products, sales, stock, and payment records.

### Limitations and known gaps

- Receipt printing has not been completed and verified for supported receipt printers.
- Data persistence across a full operating-system reboot has not been verified. In a live USB session, changes may be lost when the computer shuts down or restarts.
- Backup and restore workflows are not yet documented as production-ready.
- Tax and fiscal receipt requirements, returns/refunds, customer and supplier management, and other store workflows still need development and testing.
- Hardware compatibility has not been validated across a wide range of computers, scanners, and printers.

**Do not rely on this alpha release for real business records or daily store operations.** Use a virtual machine or spare computer, and do not store important data on it.

## Download

Open the [FastPOS OS 0.2.4-alpha release page](https://github.com/Yassine-Jemi01/fastpos/releases/tag/0.2.4-alpha) and download the ISO image and its `SHA256SUMS` file from the release assets.

If the ISO asset is not listed on the release page yet, it has not finished uploading and is not available to download.

## Verify the download

Place the downloaded ISO and `SHA256SUMS` file in the same directory, then run:

```sh
sha256sum --check SHA256SUMS
```

The result should report `OK` for the ISO. If the check fails, do not use that download; download the files again and verify them.

## Try FastPOS OS

1. Download the ISO from the release page.
2. Verify it using the checksum instructions above.
3. Write the ISO to a USB drive using an image-writing tool. Writing an image erases the selected drive, so confirm the target device carefully.
4. Boot a compatible x86_64/AMD64 computer from the USB drive, or test the image in a virtual machine.
5. Try the cashier application and manually add a few test products.

**Live-session warning:** Do not assume products or sales will remain after reboot. Reliable persistence must be confirmed for the exact installation mode before entering any data you need to keep.

## Repository contents

The default branch currently contains the FastPOS Rust core:

- `src/` — core library and development CLI.
- `tests/` — integration tests for checkout, inventory, currency, and database behavior.
- `examples/` — a small example of using the core.
- `Cargo.toml` and `Cargo.lock` — Rust package metadata and dependencies.

The bootable ISO is distributed separately through GitHub Releases. The full OS image-build configuration and graphical application source are not yet included in this repository's default branch.

## Develop and test the core

Requirements: a stable Rust toolchain and Cargo. A C build toolchain may be needed when building the bundled SQLite dependency.

Run the core test suite:

```sh
cargo test
```

Build the Rust core:

```sh
cargo build --release
```

The development CLI is intended for testing the core and is not the graphical cashier interface.

## Data model and implementation notes

The core uses SQLite and stores money and quantities as integers to avoid floating-point arithmetic for financial calculations. Its database includes product records, sales, sale items, payments, store settings, and stock movements. Checkout operations are designed to update sales and stock transactionally.

The core's default currency configuration is Tunisia (TND). Currency and country settings should be configured before adding products or sales.

## Project status

FastPOS OS is under active development. The current priority is improving data reliability, receipt printing, backups, hardware support, and the overall cashier workflow before considering production use.

Bug reports and constructive feedback are welcome through [GitHub Issues](https://github.com/Yassine-Jemi01/fastpos/issues).

---

FastPOS OS is provided under the terms of the GNU General Public License v3.0 only. See [LICENSE](LICENSE).
