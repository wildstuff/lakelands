//! The baseline of `host-changes-test.sh`: read as text, never built.

pub trait Lane {
    fn shorter(&self, decl: &lakelands_api::ontology::Decl) -> Result<()>;
    fn another(&self, decl: a::Decl) -> Result<()>;
    fn wider(&self, n: u32);
    fn renamed(&self, old: u32);
    fn missed(&self) -> Self::Out;
}

pub fn shorter(decl: &crate::ontology::Decl, all: Vec<x::Decl>) -> Result<Patch> {
    todo!()
}

pub fn borrowed(decl: Decl) -> u32 {
    0
}

pub struct Shape {
    pub shorter: lakelands_api::ontology::Decl,
    pub another: a::Decl,
    pub wider: u32,
    hidden: u8,
}

impl Shape {
    pub fn shorter(&self, key: &lakelands_api::RecordKey) -> Option<x::Decl> {
        None
    }

    pub fn another(&self, key: a::Key) {}
}

pub struct Pair(pub m::Decl, pub u8);

pub struct One { pub a: u32, pub path: m::Decl, b: u32 }

pub enum Write {
    Braced { a: u32 },
    Upsert {
        decl: m::Decl,
        count: u32,
    },
}
