"""Pydantic models for CocoIndex context extraction."""

from pydantic import BaseModel, Field


class FunctionInfo(BaseModel):
    """A public function signature extracted from a source file."""
    name: str = Field(description="Function name")
    signature: str = Field(description="Full function signature (name + params + return type)")
    summary: str = Field(description="One-line description of what it does")


class TypeInfo(BaseModel):
    """A public type (struct, enum, trait, class) extracted from a source file."""
    name: str = Field(description="Type name")
    summary: str = Field(description="One-line description of what it represents")


class FileMapInfo(BaseModel):
    """Extracted information about a single source file."""
    name: str = Field(description="File path relative to project root")
    summary: str = Field(description="One-line summary of the file's purpose")
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
