# parqeye

[![CI][actions-badge]][actions-url]
[![crates.io][crates-badge]][crates-url]
[![PyPI][pypi-badge]][pypi-url]
[![Homebrew][homebrew-badge]][homebrew-url]
[![License][license-badge]](./LICENSE)

[actions-badge]: https://github.com/kaushiksrini/parqeye/actions/workflows/ci.yaml/badge.svg
[actions-url]: https://github.com/kaushiksrini/parqeye/actions/workflows/ci.yaml
[crates-badge]: https://img.shields.io/crates/v/parqeye.svg?logo=rust
[crates-url]: https://crates.io/crates/parqeye
[pypi-badge]: https://img.shields.io/pypi/v/parqeye.svg?logo=pypi&logoColor=white
[pypi-url]: https://pypi.org/project/parqeye/
[homebrew-badge]: https://img.shields.io/homebrew/v/parqeye.svg?logo=homebrew&logoColor=white
[homebrew-url]: https://formulae.brew.sh/formula/parqeye
[license-badge]: https://img.shields.io/crates/l/parqeye.svg

`parqeye` lets you _peek inside_ your Parquet files. Instantly inspect their contents, schema, and metadata — right from your terminal.

![Demo](.github/assets/demo.gif)

**Features**

- **Interactive Data Visualization** - Browse through your Parquet data in a table view with keyboard navigation.
- **Column Graphs** - Plot the distribution of any column (histogram, density, CDF or top values) with summary statistics and automatic insights.
- **Schema Explorer** - Inspect column types, nested structures, and field definitions.
- **File Metadata** - View Parquet file-level metadata including version, created by, encoding stats and more.
- **Row Group Statistics** - Examine row group-level metadata, statistics, and data distribution across groups.
- **Tab-based Interface** - Quickly switch between Visualize, Schema, Metadata, and Row Groups views.
- **Terminal-native** - Works directly in your terminal.

# Usage

Run `parqeye` by providing the path to the `.parquet` file.

```
parqeye <path-to-parquet-file>
```

## Column graphs

In the Visualize tab, press `M` for column mode, pick a column with `←`/`→` and press `G` to plot its distribution. Numeric, date/time and decimal columns open as a histogram or a smooth density, text and booleans as top-value bars, and list columns plot the length of each list. The summary panel shows quantiles and points out skew, outliers, zero inflation, multiple peaks and likely ID columns.

| Key | Action |
| --- | --- |
| `←` / `→` | Previous / next column |
| `m`, `1`-`4` | Next view, or Histogram / Density / CDF / Top values |
| `+` / `-` | More / fewer bins, less / more smoothing |
| `x` / `y` | Log scale on the x / y axis |
| `o` | Clip the x range to the 1st-99th percentile |
| `b` / `a` | Toggle the box plot / the median and mean markers |
| `s` | Order top values by count or by value |
| `↑` / `↓` | Scroll the top values list |
| `r` | Reset the view |
| `G` / `Esc` | Back to the table |

The graph reads the whole column (only that column) the first time it is opened; later visits reuse the result.

You can also run `parqeye` using `uvx`:

```
uvx parqeye <path-to-parquet-file>
```

# Installation

## Cargo

If you use Rust, build directly from [crates.io](https://crates.io/crates/parqeye)

```
cargo install parqeye
```

## Homebrew

If you have Homebrew, you can install using:

```sh
brew install parqeye
```

## Windows

`parqeye` is available on [winget](https://learn.microsoft.com/en-gb/windows/package-manager/) for Windows. You can install using:

```powershell
winget install --id kaushiksrini.parqeye
```

## Build from Source

You can build from source by downloading the repository and running the following command:

```
cargo build --release
```

## Direct Download

You can download the latest release from the [Releases](https://github.com/kaushiksrini/parqeye/releases) page.

# License

This package is released under the [MIT License](./LICENSE).

# Acknowledgements

- [csvlens](https://github.com/YS-L/csvlens) for the inspiration


# TODOs

- [ ] Lazy/streaming loading of parquet files.
- [ ] Filter columns by value in the visualize tab.
- [ ] Read parquet files on the cloud (`s3://...`).
