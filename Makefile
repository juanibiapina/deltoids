.PHONY: install

# Install all workspace binaries to ~/.cargo/bin via `cargo install`.
# Currently the workspace ships one binary (`deltoids`) from
# `crates/deltoids-cli`. Add new `cargo install --path ...` lines here
# when more binary crates are added.
install:
	cargo install --path crates/deltoids-cli
