//! The splitting engine of Python num2words (`Num2Word_Base`). Many languages use it.
//!
//! A language gives a table of cards (a number and its word) and a `merge` rule.
//! The engine cuts the number at the largest card, and `merge` joins the parts.
//! The result is the same as num2words, so the language files can be checked
//! against it.

/// A word and the value it stands for.
pub(super) type Pair = (String, u64);

enum Node {
    Pair(Pair),
    List(Vec<Node>),
}

pub(super) struct Cards {
    /// Largest card first, as num2words keeps them.
    cards: Vec<(u64, &'static str)>,
}

impl Cards {
    /// `high` and `mid` go from large to small. `low` holds the words for 0, 1, 2 and so
    /// on, without gaps.
    pub(super) fn new(
        high: &[(u64, &'static str)],
        mid: &[(u64, &'static str)],
        low: &[&'static str],
    ) -> Self {
        let mut cards: Vec<(u64, &'static str)> = high.iter().chain(mid).copied().collect();
        cards.extend(low.iter().enumerate().rev().map(|(n, w)| (n as u64, *w)));
        Self { cards }
    }

    fn word(&self, n: u64) -> &'static str {
        self.cards.iter().find(|(k, _)| *k == n).map_or("", |c| c.1)
    }

    fn split(&self, value: u64) -> Vec<Node> {
        let &(elem, word) = self
            .cards
            .iter()
            .find(|(k, _)| *k <= value)
            .expect("card 0 always exists");
        let (div, rest) = if value == 0 {
            (1, 0)
        } else {
            (value / elem, value % elem)
        };
        let mut out = Vec::new();
        if div == 1 {
            out.push(Node::Pair((self.word(1).to_string(), 1)));
        } else {
            out.push(Node::List(self.split(div)));
        }
        out.push(Node::Pair((word.to_string(), elem)));
        if rest > 0 {
            out.push(Node::List(self.split(rest)));
        }
        out
    }

    /// The words for `value`.
    pub(super) fn cardinal(&self, value: u64, merge: fn(&Pair, &Pair) -> Pair) -> String {
        clean(self.split(value), merge).0
    }
}

fn clean(mut val: Vec<Node>, merge: fn(&Pair, &Pair) -> Pair) -> Pair {
    while val.len() != 1 {
        let mut out = Vec::new();
        if matches!((&val[0], &val[1]), (Node::Pair(_), Node::Pair(_))) {
            let mut items = val.into_iter();
            let (Some(Node::Pair(left)), Some(Node::Pair(right))) = (items.next(), items.next())
            else {
                unreachable!("checked above");
            };
            out.push(Node::Pair(merge(&left, &right)));
            let rest: Vec<Node> = items.collect();
            if !rest.is_empty() {
                out.push(Node::List(rest));
            }
        } else {
            for elem in val {
                match elem {
                    Node::List(mut list) if list.len() == 1 => out.push(list.remove(0)),
                    Node::List(list) => out.push(Node::Pair(clean(list, merge))),
                    pair => out.push(pair),
                }
            }
        }
        val = out;
    }
    match val.pop() {
        Some(Node::Pair(pair)) => pair,
        _ => unreachable!("the engine ends with one pair"),
    }
}
