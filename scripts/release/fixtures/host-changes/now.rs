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

pub enum Write {
    Upsert {
        decl: Decl,
        count: u64,
    },
}
