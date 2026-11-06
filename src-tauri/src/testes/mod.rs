//! Testes: runner de testes relacionados (X1.3) e feedback compacto de testes e build (X1.4). Funções puras.
pub mod compacto;
pub mod relacionados;

/// Saída de exemplo para os testes. `<ESC>` vira o caractere ESC, para exercitar a remoção de ANSI.
#[cfg(test)]
pub(crate) fn fixture(nome: &str) -> String {
    let t = match nome {
        "cargo_ok" => include_str!("fixtures/cargo_ok.txt"),
        "cargo_fail" => include_str!("fixtures/cargo_fail.txt"),
        "cargo_build_fail" => include_str!("fixtures/cargo_build_fail.txt"),
        "vitest_ok" => include_str!("fixtures/vitest_ok.txt"),
        "vitest_fail" => include_str!("fixtures/vitest_fail.txt"),
        "jest_ok" => include_str!("fixtures/jest_ok.txt"),
        "jest_fail" => include_str!("fixtures/jest_fail.txt"),
        "pytest_ok" => include_str!("fixtures/pytest_ok.txt"),
        "pytest_fail" => include_str!("fixtures/pytest_fail.txt"),
        "go_ok" => include_str!("fixtures/go_ok.txt"),
        "go_fail" => include_str!("fixtures/go_fail.txt"),
        "dotnet_ok" => include_str!("fixtures/dotnet_ok.txt"),
        "dotnet_fail" => include_str!("fixtures/dotnet_fail.txt"),
        "tsc_ok" => include_str!("fixtures/tsc_ok.txt"),
        "tsc_fail" => include_str!("fixtures/tsc_fail.txt"),
        "eslint_ok" => include_str!("fixtures/eslint_ok.txt"),
        "eslint_fail" => include_str!("fixtures/eslint_fail.txt"),
        outro => panic!("fixture desconhecida: {outro}"),
    };
    t.replace("<ESC>", "\u{1b}")
}
