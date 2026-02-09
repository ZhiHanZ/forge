# Design Doc Coverage Checklist

Check DESIGN.md for these 7 sections. Each missing section is a topic agents will
web-search for during implementation — filling it now saves tokens across every session.

## 1. Data Model
Structs, enums, concrete types with field names and types.

**Without this**: agents invent incompatible types across features.

**Good**: `struct User { id: Uuid, email: String, name: String, created_at: DateTime<Utc> }`
**Bad**: "We need a user model"

## 2. API Surface
Function signatures, endpoint specs, trait definitions. The contracts between scopes.

**Without this**: scope boundaries are guesswork.

**Good**: `POST /users { email, name } -> 201 { id, email, name, created_at }`
**Bad**: "RESTful API for users"

## 3. Error Strategy
Error type, how errors propagate, what gets logged vs returned.

**Without this**: every agent invents its own error handling.

**Good**: `enum AppError { NotFound(String), Unauthorized, Internal(anyhow::Error) }` with `impl IntoResponse`
**Bad**: "Handle errors gracefully"

## 4. State & Storage
Where data lives, schema, migrations, connection handling.

**Without this**: agents mix in-memory and persistent state randomly.

**Good**: "PostgreSQL via sqlx, migrations in migrations/, PgPool in app state"
**Bad**: "Use a database"

## 5. Dependencies
Which crates/libraries, version constraints, why each is chosen.

**Without this**: agents add conflicting or redundant dependencies.

**Good**: Cargo.toml with `axum = "0.7"`, `sqlx = { version = "0.8", features = ["postgres", "runtime-tokio"] }`
**Bad**: "Use standard Rust libraries"

## 6. Constraints
What NOT to do. Explicit boundaries on complexity.

**Without this**: agents over-engineer with generics, macros, or unnecessary abstractions.

**Good**: "No generics on domain types. No async traits. No ORM — raw sqlx queries only."
**Bad**: (nothing — silence is interpreted as "anything goes")

## 7. Examples
One complete request/response cycle. One complete test.

**Without this**: agents guess the project's style and conventions.

**Good**:
```rust
#[tokio::test]
async fn test_create_user() {
    let app = test_app().await;
    let res = app.post("/users").json(&json!({"email": "a@b.com", "name": "A"})).await;
    assert_eq!(res.status(), 201);
    let body: User = res.json().await;
    assert_eq!(body.email, "a@b.com");
}
```
**Bad**: "Write tests for all endpoints"
