# Rust Driver for Elevator project

To use this library add the following to `Cargo.toml`:

```toml
[dependencies]
driver-rust = { git = "https://github.com/TTK4145/driver-rust", tag = "v0.X.0" }
```

For most recent release see [releases](https://github.com/TTK4145/driver-rust/releases). Note
that we will come with breaking changes to `master`, so depending on the `master` branch directly
might lead to some issues.

When using the library in your project, it will be available under the
`driver_rust` namespace, example:

```rust
use driver_rust::elevio;

fn main() -> std::io::Result<()> {
    let num_floors = 4;
    let elevator = elevio::Elevator::init("localhost:15657", num_floors)?;
}
```

For an example of usage, see [main.rs](src/main.rs).
