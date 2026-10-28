//! The statement as a list of clauses, each with its offset, parsed by
//! recursive descent over the tokens.
//!
//! The parser knows the subset's syntax and refuses every other
//! construct at its keyword or its operator, before it reads what
//! follows, so the refusal names the construct. What the clauses mean
//! together, which name each reads and where a condition goes, is the
//! translation's ([`crate::read`]).

use std::collections::BTreeSet;

use lakelands_api::error::{Case, Construct};
use lakelands_api::expr::Expr;
use lakelands_api::query::{Depth, Walk};
use lakelands_api::{ParamName, Params, Value};

use crate::lex::{RESERVED, Tok, Token};

/// A refusal before translation: the statement does not parse, or a
/// construct is outside the subset.
pub(crate) type Result<T> = std::result::Result<T, Case>;

/// A parsed statement.
pub(crate) struct Query {
    pub explain: bool,
    pub clauses: Vec<Clause>,
}

/// One clause and the offset of its keyword.
pub(crate) struct Clause {
    pub at: usize,
    pub kind: Kind,
}

/// The clauses of the subset: reading ones, then a `RETURN` or the
/// updating ones, which the write translation ([`crate::write`]) tells
/// apart into the one door call they make.
pub(crate) enum Kind {
    Match(Match),
    Return(Return),
    /// `CREATE` or `MERGE` of its parts.
    Put {
        merge: bool,
        parts: Vec<Part>,
    },
    /// `SET` of properties, or `REMOVE` of them, each set to null.
    Set {
        remove: bool,
        items: Vec<Assign>,
    },
    /// `DELETE` of names.
    Delete(Vec<Name>),
}

/// `<name>.<field> = <value>`, or `<name>.<field>` removed.
pub(crate) struct Assign {
    pub at: usize,
    pub name: String,
    pub field: String,
    pub value: Value,
}

/// `[OPTIONAL] MATCH <parts> [WHERE <condition>]`.
pub(crate) struct Match {
    pub optional: bool,
    pub parts: Vec<Part>,
    pub filter: Option<Where>,
}

/// A condition, the names it reads in the order it reads them, and the
/// parts it says must not match. Each such part stands in `expr` as a
/// marker ([`absent_marker`]), so the translation tells whether it is a
/// whole conjunct.
pub(crate) struct Where {
    pub expr: Expr,
    pub names: Vec<Name>,
    pub absent: Vec<Absent>,
}

/// A variable an expression reads, and where.
#[derive(Clone)]
pub(crate) struct Name {
    pub name: String,
    pub at: usize,
}

/// `NOT EXISTS { [MATCH] <part> [WHERE <condition>] }`, or `NOT <part>`.
pub(crate) struct Absent {
    pub at: usize,
    pub part: Part,
    pub filter: Option<Where>,
}

/// A path pattern: nodes joined by relationships, one more node than
/// relationships.
pub(crate) struct Part {
    pub nodes: Vec<Node>,
    pub rels: Vec<Rel>,
}

/// `(<var>:<label> {<props>})`, each part optional.
pub(crate) struct Node {
    pub at: usize,
    pub var: Option<String>,
    pub label: Option<String>,
    pub props: Vec<Prop>,
}

/// A property of a node's map: its key and value, kept as one, and the
/// parameter it was if it was one.
pub(crate) struct Prop {
    pub at: usize,
    pub key: String,
    pub value: Expr,
    pub names: Vec<Name>,
    pub param: Option<String>,
}

/// `-[<var>:<relation>*<range> {<props>}]->`, `<-[…]-` or `-[…]-`; a
/// read takes no variable and no map, which a write names a link by.
pub(crate) struct Rel {
    pub at: usize,
    pub var: Option<Name>,
    pub props: Option<(usize, Vec<Prop>)>,
    pub walk: Walk,
    pub relation: String,
    pub depth: Depth,
}

/// `RETURN <items> [ORDER BY <keys>] [SKIP 0] [LIMIT <n>]`.
pub(crate) struct Return {
    pub star: bool,
    pub items: Vec<Item>,
    pub order: Vec<Sort>,
    pub limit: Option<u32>,
}

/// A returned item: its expression, its `AS` and its text as written.
pub(crate) struct Item {
    pub expr: Expr,
    pub alias: Option<String>,
    pub text: String,
    pub names: Vec<Name>,
    /// Where each aggregate call within it stands.
    pub aggregates: Vec<usize>,
}

/// A sort key and its direction.
pub(crate) struct Sort {
    pub expr: Expr,
    pub text: String,
    pub names: Vec<Name>,
    pub desc: bool,
}

/// The parser's state: the tokens, the statement's characters for an
/// item's text, the parameters and which of them were read, and what
/// the expression under way reads, and how deep it nests.
pub(crate) struct Parser<'p> {
    pub depth: usize,
    pub toks: Vec<Token>,
    pub i: usize,
    pub chars: Vec<char>,
    pub params: &'p Params,
    pub used: BTreeSet<ParamName>,
    pub names: Vec<Name>,
    pub aggregates: Vec<usize>,
    pub absent: Vec<Absent>,
    /// The first updating clause, once one is read.
    pub wrote: Option<(Construct, usize)>,
}

/// Each updating clause the subset takes, by its keyword.
const WRITES: [(&str, Construct); 5] = [
    ("CREATE", Construct::Create),
    ("MERGE", Construct::Merge),
    ("SET", Construct::Set),
    ("REMOVE", Construct::Remove),
    ("DELETE", Construct::Delete),
];

/// Each clause outside the subset, by the keyword it begins with.
const CLAUSES: [(&str, Construct); 9] = [
    ("DETACH", Construct::DetachDelete),
    ("DROP", Construct::Schema),
    ("FOREACH", Construct::Foreach),
    ("LOAD", Construct::LoadCsv),
    ("CALL", Construct::Call),
    ("WITH", Construct::With),
    ("UNION", Construct::Union),
    ("UNWIND", Construct::Unwind),
    ("USE", Construct::Use),
];

/// The marker a part that must not match stands as in its condition:
/// `exists` is a keyword of the statement, so no call of it is written.
pub(crate) fn absent_marker(index: usize) -> Expr {
    let number = index.to_string();
    Expr::Call {
        func: lakelands_api::expr::Func::Exists,
        args: vec![Expr::Lit {
            value: lakelands_api::expr::Literal::Number(number),
        }],
    }
}

/// The construct refused at `offset`.
pub(crate) fn outside(construct: Construct, offset: usize) -> Case {
    Case::CypherOutside { construct, offset }
}

impl<'p> Parser<'p> {
    pub fn new(toks: Vec<Token>, chars: Vec<char>, params: &'p Params) -> Self {
        Parser {
            depth: 0,
            toks,
            i: 0,
            chars,
            params,
            used: BTreeSet::new(),
            names: Vec::new(),
            aggregates: Vec::new(),
            absent: Vec::new(),
            wrote: None,
        }
    }

    pub fn tok(&self) -> &Tok {
        &self.toks[self.i].tok
    }

    pub fn at(&self) -> usize {
        self.toks[self.i].at
    }

    pub fn ahead(&self, n: usize) -> &Tok {
        &self.toks[(self.i + n).min(self.toks.len() - 1)].tok
    }

    pub fn bump(&mut self) {
        if self.i + 1 < self.toks.len() {
            self.i += 1;
        }
    }

    /// Whether the token `n` ahead is the keyword `kw`, in any case.
    pub fn is_kw_at(&self, n: usize, kw: &str) -> bool {
        matches!(self.ahead(n), Tok::Word(w) if w.eq_ignore_ascii_case(kw))
    }

    pub fn is_kw(&self, kw: &str) -> bool {
        self.is_kw_at(0, kw)
    }

    pub fn kw(&mut self, kw: &str) -> bool {
        let is = self.is_kw(kw);
        if is {
            self.bump();
        }
        is
    }

    pub fn is(&self, p: &str) -> bool {
        matches!(self.tok(), Tok::Punct(q) if *q == p)
    }

    pub fn eat(&mut self, p: &str) -> bool {
        let is = self.is(p);
        if is {
            self.bump();
        }
        is
    }

    /// The token `token` expected here.
    pub fn expected(&self, token: &str) -> Case {
        if matches!(self.tok(), Tok::End) && !matches!(token, ")" | "]" | "}") {
            return Case::ParseEndsEarly { offset: self.at() };
        }
        let (token, offset) = (token.to_owned(), self.at());
        Case::ParseExpected { token, offset }
    }

    pub fn expect(&mut self, p: &str) -> Result<()> {
        if self.eat(p) {
            Ok(())
        } else {
            Err(self.expected(p))
        }
    }

    pub fn expect_kw(&mut self, kw: &str) -> Result<()> {
        if self.kw(kw) {
            Ok(())
        } else {
            Err(self.expected(kw))
        }
    }

    /// A name: a word that is no reserved word, or a backticked one;
    /// `schema` also takes a reserved word, as a label, a relationship
    /// type or a property key may be one.
    pub fn name(&mut self, schema: bool) -> Result<String> {
        let at = self.at();
        let name = match self.tok().clone() {
            Tok::Quoted(n) => n,
            Tok::Word(w) if schema || !reserved(&w) => w,
            Tok::Word(w) => {
                let keyword = w.to_ascii_uppercase();
                return Err(Case::ParseKeyword {
                    keyword,
                    offset: at,
                });
            }
            _ => return Err(self.expected("a name")),
        };
        self.bump();
        Ok(name)
    }

    /// The text written from offset `from` to the token before this one.
    pub fn text_from(&self, from: usize) -> String {
        let end = self.toks[self.i.saturating_sub(1)].end.max(from);
        self.chars[from..end].iter().collect()
    }

    /// The whole statement.
    pub fn query(&mut self) -> Result<Query> {
        if self.is_kw("PROFILE") {
            return Err(outside(Construct::Profile, self.at()));
        }
        let explain = self.at();
        let explain = self.kw("EXPLAIN").then_some(explain);
        let mut clauses = Vec::new();
        let mut writes = false;
        loop {
            let at = self.at();
            let write = WRITES.iter().find(|(kw, _)| self.is_kw(kw));
            if self.is_kw("MATCH") || (self.is_kw("OPTIONAL") && !clauses.is_empty()) {
                let optional = self.kw("OPTIONAL");
                self.expect_kw("MATCH")?;
                let kind = Kind::Match(self.matching(optional)?);
                clauses.push(Clause { at, kind });
            } else if self.is_kw("RETURN") && writes {
                return Err(outside(Construct::ReturnBesideWrite, at));
            } else if self.is_kw("RETURN") && !clauses.is_empty() {
                self.bump();
                let kind = Kind::Return(self.returning()?);
                clauses.push(Clause { at, kind });
                break;
            } else if let Some((kw, construct)) = write {
                self.wrote.get_or_insert((construct.clone(), at));
                if let Some(at) = explain {
                    return Err(outside(Construct::ExplainWrite, at));
                }
                self.bump();
                let kind = self.writing(kw, at)?;
                clauses.push(Clause { at, kind });
                writes = true;
            } else if writes && matches!(self.tok(), Tok::End | Tok::Punct(";")) {
                break;
            } else {
                return Err(self.no_clause(clauses.is_empty()));
            }
        }
        let explain = explain.is_some();
        self.eat(";");
        if !matches!(self.tok(), Tok::End) {
            let construct = match self.toks[self.i - 1].tok {
                Tok::Punct(";") => Construct::SeveralStatements,
                _ => return Err(self.no_clause(false)),
            };
            return Err(outside(construct, self.at()));
        }
        Ok(Query { explain, clauses })
    }

    /// What stands where a clause does not: a clause outside the subset,
    /// or a token that begins none.
    fn no_clause(&self, first: bool) -> Case {
        let at = self.at();
        if let Some((_, c)) = CLAUSES.iter().find(|(kw, _)| self.is_kw(kw)) {
            return outside(c.clone(), at);
        }
        if self.is_kw("USING") {
            return outside(Construct::Using, at);
        }
        match self.tok() {
            Tok::End if !first => self.expected("RETURN"),
            _ if first => self.expected("MATCH"),
            _ => Case::ParseTrailing { offset: at },
        }
    }

    /// The updating clause `kw`, after its keyword at `at`.
    fn writing(&mut self, kw: &str, at: usize) -> Result<Kind> {
        if kw == "DELETE" {
            let mut names = Vec::new();
            loop {
                let at = self.at();
                let name = self.name(false)?;
                names.push(Name { name, at });
                if !self.eat(",") {
                    return Ok(Kind::Delete(names));
                }
            }
        }
        if kw == "SET" || kw == "REMOVE" {
            let remove = kw == "REMOVE";
            let items = self.assigns(remove)?;
            return Ok(Kind::Set { remove, items });
        }
        if !self.is("(") {
            return Err(outside(Construct::Schema, at));
        }
        let mut parts = vec![self.part()?];
        while self.eat(",") {
            parts.push(self.part()?);
        }
        if self.is_kw("ON") {
            let on = match self.is_kw_at(1, "CREATE") {
                true => Construct::OnCreate,
                false => Construct::OnMatch,
            };
            return Err(outside(on, self.at()));
        }
        let merge = kw == "MERGE";
        Ok(Kind::Put { merge, parts })
    }

    /// `SET`'s items, or `REMOVE`'s, each set to null.
    fn assigns(&mut self, remove: bool) -> Result<Vec<Assign>> {
        let mut items = Vec::new();
        loop {
            let at = self.at();
            let name = self.name(false)?;
            if self.is(":") {
                return Err(outside(Construct::Label, self.at()));
            }
            if !remove && (self.is("=") || self.is("+")) {
                return Err(outside(Construct::SetMap, at));
            }
            self.expect(".")?;
            let field = self.name(true)?;
            let value = match remove {
                true => Value::Null,
                false => {
                    self.expect("=")?;
                    self.value()?
                }
            };
            items.push(Assign {
                at,
                name,
                field,
                value,
            });
            if !self.eat(",") {
                return Ok(items);
            }
        }
    }

    /// A value written: a literal, or a parameter of any value; one
    /// computed or read from a record is refused.
    pub fn value(&mut self) -> Result<Value> {
        let at = self.at();
        if let Tok::Param(p) = self.tok().clone() {
            let name = ParamName::from(p);
            let v = self.param(&name)?;
            if v.is_array() || v.is_object() {
                self.bump();
                return Ok(v);
            }
        }
        let w = self.condition()?;
        if let Some(a) = w.absent.first() {
            return Err(outside(Construct::Exists, a.at));
        }
        literal(&w.expr).ok_or(outside(Construct::ComputedValue, at))
    }

    fn matching(&mut self, optional: bool) -> Result<Match> {
        let mut parts = vec![self.part()?];
        while self.eat(",") {
            parts.push(self.part()?);
        }
        if self.is_kw("USING") {
            return Err(outside(Construct::Using, self.at()));
        }
        let filter = if self.kw("WHERE") {
            Some(self.condition()?)
        } else {
            None
        };
        Ok(Match {
            optional,
            parts,
            filter,
        })
    }

    /// A condition, with what it reads.
    pub fn condition(&mut self) -> Result<Where> {
        let outer = (
            std::mem::take(&mut self.names),
            std::mem::take(&mut self.absent),
        );
        let expr = self.or();
        let names = std::mem::replace(&mut self.names, outer.0);
        let absent = std::mem::replace(&mut self.absent, outer.1);
        Ok(Where {
            expr: expr?,
            names,
            absent,
        })
    }

    /// One path pattern.
    pub fn part(&mut self) -> Result<Part> {
        let at = self.at();
        if matches!(self.tok(), Tok::Word(_) | Tok::Quoted(_)) {
            let word = matches!(self.tok(), Tok::Word(w) if is_shortest(w));
            if word && matches!(self.ahead(1), Tok::Punct("(")) {
                return Err(outside(Construct::ShortestPath, at));
            }
            if matches!(self.ahead(1), Tok::Punct("=")) {
                return Err(outside(Construct::NamedPath, at));
            }
        }
        let mut nodes = vec![self.node()?];
        let mut rels = Vec::new();
        while self.is("-") || self.is("<") {
            rels.push(self.rel()?);
            nodes.push(self.node()?);
        }
        Ok(Part { nodes, rels })
    }

    fn node(&mut self) -> Result<Node> {
        let at = self.at();
        self.expect("(")?;
        let var = match self.tok() {
            Tok::Word(_) | Tok::Quoted(_) => Some(self.name(false)?),
            _ => None,
        };
        let label = if self.eat(":") {
            Some(self.name(true)?)
        } else {
            None
        };
        if self.is(":") || self.is("|") {
            return Err(outside(Construct::SeveralLabels, self.at()));
        }
        let props = if self.is("{") {
            self.props()?
        } else if let Tok::Param(_) = self.tok() {
            return Err(outside(Construct::MapLiteral, self.at()));
        } else {
            Vec::new()
        };
        self.expect(")")?;
        Ok(Node {
            at,
            var,
            label,
            props,
        })
    }

    /// A node's property map, kept as a map of name to expression.
    fn props(&mut self) -> Result<Vec<Prop>> {
        self.expect("{")?;
        let mut props = Vec::new();
        if self.eat("}") {
            return Ok(props);
        }
        loop {
            let key = self.name(true)?;
            self.expect(":")?;
            let at = self.at();
            let param = match self.tok() {
                Tok::Param(p) if matches!(self.ahead(1), Tok::Punct("," | "}")) => Some(p.clone()),
                _ => None,
            };
            let w = self.condition()?;
            if let Some(a) = w.absent.first() {
                return Err(outside(Construct::Exists, a.at));
            }
            props.push(Prop {
                at,
                key,
                value: w.expr,
                names: w.names,
                param,
            });
            if self.eat("}") {
                return Ok(props);
            }
            self.expect(",")?;
        }
    }

    fn rel(&mut self) -> Result<Rel> {
        let at = self.at();
        let left = self.eat("<");
        self.expect("-")?;
        if !self.is("[") {
            return Err(self.expected("["));
        }
        self.bump();
        let mut var = None;
        if matches!(self.tok(), Tok::Word(_) | Tok::Quoted(_)) {
            let at = self.at();
            if !matches!(self.ahead(1), Tok::Punct(":")) {
                return Err(outside(Construct::RelationshipVariable, at));
            }
            let name = self.name(false)?;
            var = Some(Name { name, at });
        }
        self.expect(":")?;
        let relation = self.name(true)?;
        if self.is("|") {
            return Err(outside(Construct::RelationAlternation, self.at()));
        }
        let depth = if self.is("*") {
            self.range()?
        } else {
            Depth::default()
        };
        let props = match self.tok() {
            Tok::Punct("{") => Some((self.at(), self.props()?)),
            Tok::Param(_) => return Err(outside(Construct::RelationshipProperties, self.at())),
            _ => None,
        };
        self.expect("]")?;
        self.expect("-")?;
        let right = self.eat(">");
        let walk = match (left, right) {
            (false, true) => Walk::Out,
            (true, false) => Walk::In,
            _ => Walk::Both,
        };
        Ok(Rel {
            at,
            var,
            props,
            walk,
            relation,
            depth,
        })
    }

    /// `*n`, `*m..n` or `*..n`; a range with no upper bound is refused.
    fn range(&mut self) -> Result<Depth> {
        let star = self.at();
        self.bump();
        let unbounded = || outside(Construct::UnboundedLength, star);
        let min = self.whole()?;
        if !self.eat("..") {
            let n = min.ok_or_else(unbounded)?;
            return Ok(Depth { min: n, max: n });
        }
        let max = self.whole()?.ok_or_else(unbounded)?;
        Ok(Depth {
            min: min.unwrap_or(1),
            max,
        })
    }

    /// A whole number of hops, if one is written here.
    fn whole(&mut self) -> Result<Option<u32>> {
        let Tok::Num(n) = self.tok() else {
            return Ok(None);
        };
        let at = self.at();
        // A depth past `u32` is past every `max_depth` too; the store
        // refuses it as it refuses any depth above that.
        let n = match n.parse::<u64>() {
            Ok(n) => u32::try_from(n).unwrap_or(u32::MAX),
            Err(_) if n.chars().all(|c| c.is_ascii_digit()) => u32::MAX,
            Err(_) => {
                let (token, offset) = ("..".to_owned(), at);
                return Err(Case::ParseExpected { token, offset });
            }
        };
        self.bump();
        Ok(Some(n))
    }

    fn returning(&mut self) -> Result<Return> {
        if self.is_kw("DISTINCT") {
            return Err(outside(Construct::Distinct, self.at()));
        }
        let star = self.eat("*");
        let mut items = Vec::new();
        if !star {
            loop {
                items.push(self.item()?);
                if !self.eat(",") {
                    break;
                }
            }
        }
        let mut order = Vec::new();
        let ordered = self.at();
        if self.kw("ORDER") {
            // An aggregate's groups come in one order, by the fields
            // beside it: an order of the statement's own is refused here,
            // in its words, before the pattern would refuse its own.
            if items.iter().any(|i| !i.aggregates.is_empty()) {
                return Err(outside(Construct::OrderByAggregate, ordered));
            }
            self.expect_kw("BY")?;
            loop {
                let from = self.at();
                let w = self.condition()?;
                if let Some(a) = w.absent.first() {
                    return Err(outside(Construct::Exists, a.at));
                }
                let text = self.text_from(from);
                let desc = self.kw("DESC") || self.kw("DESCENDING");
                if !desc && !self.kw("ASC") {
                    self.kw("ASCENDING");
                }
                order.push(Sort {
                    expr: w.expr,
                    text,
                    names: w.names,
                    desc,
                });
                if !self.eat(",") {
                    break;
                }
            }
        }
        if self.kw("SKIP") {
            let skip = self.count("SKIP")?;
            if skip != 0 {
                return Err(Case::CypherSkip { skip });
            }
        }
        let limit = if self.kw("LIMIT") {
            // A limit past `u32` is past the store's range, which it
            // refuses as it refuses any limit above its ceiling.
            Some(u32::try_from(self.count("LIMIT")?).unwrap_or(u32::MAX))
        } else {
            None
        };
        Ok(Return {
            star,
            items,
            order,
            limit,
        })
    }

    fn item(&mut self) -> Result<Item> {
        let from = self.at();
        let outer = std::mem::take(&mut self.aggregates);
        let w = self.condition()?;
        if let Some(a) = w.absent.first() {
            return Err(outside(Construct::Exists, a.at));
        }
        let text = self.text_from(from);
        let aggregates = std::mem::replace(&mut self.aggregates, outer);
        let alias = if self.kw("AS") {
            Some(self.name(false)?)
        } else {
            None
        };
        Ok(Item {
            expr: w.expr,
            alias,
            text,
            names: w.names,
            aggregates,
        })
    }

    /// The whole number `SKIP` or `LIMIT` takes, written or a parameter.
    /// Digits past `u64` stand at its maximum, past any limit the store
    /// takes.
    fn count(&mut self, clause: &str) -> Result<u64> {
        let n = match self.tok().clone() {
            Tok::Num(n) if n.bytes().all(|b| b.is_ascii_digit()) => {
                n.parse::<u64>().unwrap_or(u64::MAX)
            }
            Tok::Param(p) => {
                let name = ParamName::from(p);
                let value = self.param(&name)?;
                value.as_u64().ok_or(Case::CypherParamValue { name })?
            }
            Tok::End => return Err(Case::ParseEndsEarly { offset: self.at() }),
            _ => {
                let (clause, offset) = (clause.to_owned(), self.at());
                return Err(Case::CypherWhole { clause, offset });
            }
        };
        self.bump();
        Ok(n)
    }

    /// A parameter's value, marked as read.
    pub fn param(&mut self, name: &ParamName) -> Result<lakelands_api::Value> {
        let value = self.params.get(name).cloned();
        let value = value.ok_or_else(|| Case::CypherParamMissing { name: name.clone() })?;
        self.used.insert(name.clone());
        Ok(value)
    }
}

/// The value a literal is, a negative number's too; `None` for any
/// other expression, or a duration, which a field takes as written.
pub(crate) fn literal(e: &Expr) -> Option<Value> {
    use lakelands_api::expr::Literal;
    match e {
        Expr::Lit { value } => match value {
            Literal::Null => Some(Value::Null),
            Literal::Bool(b) => Some(Value::Bool(*b)),
            Literal::Number(n) => serde_json::from_str(n).ok(),
            Literal::Text(t) => Some(Value::String(t.clone())),
            _ => None,
        },
        Expr::Neg { arg } => match &**arg {
            Expr::Lit {
                value: Literal::Number(n),
            } => serde_json::from_str(&format!("-{n}")).ok(),
            _ => None,
        },
        _ => None,
    }
}

/// Whether a word is a reserved word of openCypher.
pub(crate) fn reserved(word: &str) -> bool {
    RESERVED.iter().any(|r| r.eq_ignore_ascii_case(word))
}

fn is_shortest(word: &str) -> bool {
    word.eq_ignore_ascii_case("shortestPath") || word.eq_ignore_ascii_case("allShortestPaths")
}
