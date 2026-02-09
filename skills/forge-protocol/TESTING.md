# 7 Rules of Meaningful Tests

Follow these rules when writing tests. They define what "prove code works" means.

## 1. Test behavior, not implementation

Call public API, assert return values and side effects. Never assert internal state.

```rust
// Good: tests behavior
assert_eq!(calculator.total(), 100);

// Bad: tests implementation detail
assert_eq!(calculator._cache["x"], 5);
```

If refactoring breaks the test, the test was wrong.

## 2. Arrange-Act-Assert

Every test has three parts: setup, one action, one assertion group. Test one thing at a time.

```rust
// Arrange
let parser = Parser::new(config);

// Act
let result = parser.parse("input");

// Assert
assert_eq!(result.tokens.len(), 3);
```

## 3. Name by business logic

`parse_thrift_rejects_invalid_version`, not `test_parse`. A failing test name alone
should tell you what requirement broke.

## 4. Target edge cases and boundaries

- Empty/null inputs
- Boundary values (if discount at $100, test $99/$100/$101)
- Error states (correct error type, not just "didn't crash")

## 5. Keep isolated and deterministic

- No dependency on test order
- No network or database calls
- No global mutable state
- Mock external dependencies

## 6. Refactoring confidence

Can you refactor internal logic without changing tests? Yes = meaningful tests.
No = change-detector tests that need rewriting.

## 7. Coverage as tool, not goal

Don't aim for a number. Use coverage reports to find gaps in critical logic —
the paths that would hurt users if broken.
