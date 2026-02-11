"""Pydantic models for CocoIndex context extraction."""

from pydantic import BaseModel, Field


class FunctionInfo(BaseModel):
    """A public function signature extracted from a source file."""
    name: str = Field(description="Function name")
    signature: str = Field(
        description="Full function signature, e.g. 'pub fn foo(x: &str) -> Result<Bar>'"
    )
    is_entry_point: bool = Field(
        default=False,
        description="Whether this is a framework entry point (main, handler, test, decorated)",
    )
    summary: str = Field(description="Brief summary of what the function does")


class TypeInfo(BaseModel):
    """A public type (struct, enum, trait, class, interface) extracted from a source file."""
    name: str = Field(description="Type name")
    kind: str = Field(
        default="",
        description="Kind of type: struct, enum, trait, class, interface, type alias",
    )
    summary: str = Field(description="Brief summary of what it represents")


class FileMapInfo(BaseModel):
    """Extracted information about a single source file."""
    name: str = Field(description="File path relative to project root")
    summary: str = Field(description="Brief summary of purpose and functionality")
    lines: int = Field(description="Total line count")
    public_functions: list[FunctionInfo] = Field(
        default_factory=list,
        description="Public functions/methods in this file",
    )
    public_types: list[TypeInfo] = Field(
        default_factory=list,
        description="Public types (structs, enums, traits, classes) in this file",
    )
    key_imports: list[str] = Field(
        default_factory=list,
        description="Important imports/dependencies used by this file",
    )
    mermaid_graphs: list[str] = Field(
        default_factory=list,
        description="Mermaid graphs showing function call relationships or type hierarchies",
    )
