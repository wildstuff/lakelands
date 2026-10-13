//! The head of `host-changes-test.sh`: read as text, never built.

pub trait Lane {
    fn shorter(&self, decl: &Decl) -> Result<()>;
    fn another(&self, decl: b::Decl) -> Result<()>;
    fn wider(&self, n: u64);
    fn renamed(&self, new: u32);
    fn missed(&self) -> Out;
}

pub fn shorter(decl: &Decl, all: Vec<Decl>) -> Result<Patch> {
    todo!()
}

pub fn borrowed(decl: &Decl) -> u32 {
    0
}

pub struct Shape {
    pub shorter: Decl,
    pub another: b::Decl,
    pub wider: u64,
    hidden: u16,
}

impl Shape {
    pub fn shorter(&self, key: &RecordKey) -> Option<Decl> {
        None
    }

    pub fn another(&self, key: b::Key) {}
}

pub struct Pair(pub Decl, pub u16);

pub struct One { pub a: u64, pub path: Decl, b: u64 }

pub enum Write {
    Braced { a: u64 },
    Upsert {
        decl: Decl,
        count: u64,
    },
}

#[doc(hidden)]
pub trait Shared {
    fn wider(&self, n: u64);
}

/// Shared with the workspace's crates, no host's.
#[doc(hidden)]
pub fn shared(n: u64) -> u32 {
    0
}

#[doc(hidden)]
#[derive(Clone)]
pub struct Inner {
    pub wider: u64,
}

impl Shape {
    #[doc(hidden)]
    pub fn shared(&self, n: u64) {}
}
