# Bitcasky

Bitcasky is a Rust implementation of the Bitcask key-value store. Bitcask is an ACID-compliant, append-only key-value store that provides high write throughput. It is optimized for write-heavy workloads, and is commonly used for applications such as log storage and time-series data.

## Features

- Append-only storage for durability and consistency
- Memory-mapped files for efficient I/O
- Key-value storage with O(1) read and write performance

## Usage

To use Bitcasky, simply add it to your `Cargo.toml` file:

```
[dependencies]
bitcasky = "0.1.0"

```

Then, in your Rust code, import the `bitcasky` crate and start using the key-value store:

```
use bitcasky::Bitcasky;

fn main() {
    let mut db = Bitcasky::open("/path/to/db", BitcaskyOptions::default()).unwrap()

    db.put(b"key", b"value").unwrap();
    let value = db.get(b"key").unwrap();

    println!("{:?}", value);
}

```
