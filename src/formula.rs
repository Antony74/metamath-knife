//! `Formula` stores the result of a parsing as the tree of its "syntactic proof"
//! The formula nodes are the equivalent of MMJ2's `ParseNode`s, and the formula
//! itself the equivalent of MMJ2's `ParseTree`

// There are several improvements which could be made to this implementation,
// without changing the API:
//
// - `sub_eq`:
//      We could compute a hash of a formula and store it in every node, to
//      speed up equality testing.
// - `substitute`:
//      A more advanced implementation of `substitute` may act directly on the
//      slice backing the formula to first copy in bulk the formula tree, which
//      will remain mostly intact, then the substitutions, and then
//      only change the nodes where the formula points to the substitutions.
//      It would even be possible to reuse the nodes, pointing several times
//      to the same node if a substituted variable appears several times
//      in the formula to be substituted.

use crate::bit_set::Bitset;
use crate::nameck::Atom;
use crate::nameck::Nameset;
use crate::parser::as_str;
use crate::parser::SymbolType;
use crate::parser::TokenIter;
use crate::scopeck::Hyp;
use crate::segment_set::SegmentSet;
use crate::tree::NodeId;
use crate::tree::SiblingIter;
use crate::tree::Tree;
use crate::util::fast_extend;
use crate::util::new_map;
use crate::util::HashMap;
use crate::verify::ProofBuilder;
use crate::Database;
use core::ops::Index;
use std::fmt::Debug;
use std::fmt::Display;
use std::iter::FromIterator;
use std::ops::Range;
use std::sync::Arc;

/// An atom representing a typecode (for "set.mm", that's one of 'wff', 'class', 'setvar' or '|-')
pub type TypeCode = Atom;

/// An atom representing a math symbol
pub type Symbol = Atom;

/// An atom representing a label (nameck suggests `LAtom` for this)
pub type Label = Atom;

#[derive(Clone, Default)]
/// A set of substitutions, mapping variables to a formula
/// We also could have used `dyn Index<&Label, Output=Box<Formula>>`
#[derive(Debug)]
pub struct Substitutions(HashMap<Label, Formula>);

impl Index<Label> for Substitutions {
    type Output = Formula;

    #[inline]
    fn index(&self, label: Label) -> &Self::Output {
        &self.0[&label]
    }
}

impl Substitutions {
    /// Inserts a substitution into the substitution set.
    pub fn insert(&mut self, label: Label, formula: Formula) -> Option<Formula> {
        self.0.insert(label, formula)
    }

    /// Add all the provided substitutions to this one
    pub fn extend(&mut self, substitutions: &Substitutions) {
        self.0.extend(substitutions.0.clone());
    }
}

/// A parsed formula, in a tree format which is convenient to perform unifications
#[derive(Clone, Default, Debug)]
pub struct Formula {
    typecode: TypeCode,
    tree: Arc<Tree<Label>>,
    root: NodeId,
    variables: Bitset,
}

impl Formula {
    /// Augment a formula with a database reference, to produce a [`FormulaRef`].
    /// The resulting object implements [`Display`], [`Debug`], and [`IntoIterator`].
    #[must_use]
    pub const fn as_ref<'a>(&'a self, db: &'a Database) -> FormulaRef<'a> {
        FormulaRef { db, formula: self }
    }

    /// Debug only, dumps the internal structure of the formula.
    pub fn dump(&self, nset: &Nameset) {
        println!("  Root: {}", self.root);
        self.tree.dump(|atom| as_str(nset.atom_name(*atom)));
    }

    /// Returns the label obtained when following the given path.
    /// Each element of the path gives the index of the child to retrieve.
    /// For example, the empty
    #[must_use]
    pub fn get_by_path(&self, path: &[usize]) -> Option<Label> {
        let mut node_id = self.root;
        for index in path {
            node_id = self.tree.nth_child(node_id, *index)?;
        }
        Some(self.tree[node_id])
    }

    #[inline]
    /// Returns whether the node given by `node_id` is a variable.
    fn is_variable(&self, node_id: NodeId) -> bool {
        self.variables.has_bit(node_id)
    }

    /// Returns a subformula, with its root at the given `node_id`
    fn sub_formula(&self, node_id: NodeId) -> Formula {
        Formula {
            typecode: self.typecode, // TODO(tirix)
            tree: self.tree.clone(),
            root: node_id,
            variables: self.variables.clone(),
        }
    }

    /// Check for equality of sub-formulas
    fn sub_eq(&self, node_id: NodeId, other: &Formula, other_node_id: NodeId) -> bool {
        (Arc::ptr_eq(&self.tree, &other.tree) && node_id == other_node_id)
            || (self.tree[node_id] == other.tree[other_node_id]
                && self.tree.has_children(node_id) == other.tree.has_children(other_node_id)
                && self
                    .tree
                    .children_iter(node_id)
                    .zip(other.tree.children_iter(other_node_id))
                    .all(|(s_id, o_id)| self.sub_eq(s_id, other, o_id)))
    }

    /// Unify this formula with the given formula model
    /// If successful, this returns the substitutions which needs to be made in
    /// `other` in order to match this formula.
    #[must_use]
    pub fn unify(&self, other: &Formula) -> Option<Box<Substitutions>> {
        let mut substitutions = Substitutions(new_map());
        self.sub_unify(self.root, other, other.root, &mut substitutions)?;
        Some(Box::new(substitutions))
    }

    /// Unify a sub-formula
    fn sub_unify(
        &self,
        node_id: NodeId,
        other: &Formula,
        other_node_id: NodeId,
        substitutions: &mut Substitutions,
    ) -> Option<()> {
        if other.is_variable(other_node_id) {
            // the model formula is a variable, build or match the substitution
            if let Some(formula) = substitutions.0.get(&other.tree[other_node_id]) {
                // there already is as substitution for that variable, check equality
                self.sub_eq(node_id, formula, formula.root).then(|| {})
            } else {
                // store the new substitution and succeed
                substitutions
                    .0
                    .insert(other.tree[other_node_id], self.sub_formula(node_id));
                Some(())
            }
        } else if self.tree[node_id] == other.tree[other_node_id]
            && self.tree.has_children(node_id) == other.tree.has_children(other_node_id)
        {
            // same nodes, we compare all children nodes
            for (s_id, o_id) in self
                .tree
                .children_iter(node_id)
                .zip(other.tree.children_iter(other_node_id))
            {
                self.sub_unify(s_id, other, o_id, substitutions)?;
            }
            Some(())
        } else {
            // formulas differ, we cannot unify.
            None
        }
    }

    /// Perform substitutions
    /// This returns a new `Formula` object, built from this formula,
    /// where all instances of the variables specified in the substitutions are
    /// replaced by the corresponding formulas.
    #[must_use]
    pub fn substitute(&self, substitutions: &Substitutions) -> Formula {
        let mut formula_builder = FormulaBuilder::default();
        self.sub_substitute(self.root, substitutions, &mut formula_builder);
        formula_builder.build(self.typecode)
    }

    /// Perform substitutions on a sub-formula, starting from the given `node_id`
    // TODO(tirix): shall we enforce that *all* variables occurring in this formula have a substitution?
    fn sub_substitute(
        &self,
        node_id: NodeId,
        substitutions: &Substitutions,
        formula_builder: &mut FormulaBuilder,
    ) {
        // TODO(tirix): use https://rust-lang.github.io/rfcs/2497-if-let-chains.html once it's out!
        if self.is_variable(node_id) {
            if let Some(formula) = substitutions.0.get(&self.tree[node_id]) {
                // We encounter a variable, perform substitution.
                formula.copy_sub_formula(formula.root, formula_builder);
                return;
            }
        }
        let mut children_count = 0;
        for child_node_id in self.tree.children_iter(node_id) {
            self.sub_substitute(child_node_id, substitutions, formula_builder);
            children_count += 1;
        }
        formula_builder.reduce(
            self.tree[node_id],
            children_count,
            0,
            self.is_variable(node_id),
        );
    }

    // Copy a sub-formula of this formula to a formula builder
    fn copy_sub_formula(&self, node_id: NodeId, formula_builder: &mut FormulaBuilder) {
        let mut children_count = 0;
        for child_node_id in self.tree.children_iter(node_id) {
            self.copy_sub_formula(child_node_id, formula_builder);
            children_count += 1;
        }
        formula_builder.reduce(
            self.tree[node_id],
            children_count,
            0,
            self.is_variable(node_id),
        );
    }
}

impl PartialEq for Formula {
    fn eq(&self, other: &Self) -> bool {
        self.sub_eq(self.root, other, other.root)
    }
}

/// A [`Formula`] reference in the context of a [`Database`].
/// This allows the values in the [`Formula`] to be resolved,
#[derive(Copy, Clone)]
pub struct FormulaRef<'a> {
    db: &'a Database,
    formula: &'a Formula,
}

impl<'a> std::ops::Deref for FormulaRef<'a> {
    type Target = Formula;

    fn deref(&self) -> &Self::Target {
        self.formula
    }
}

impl<'a> FormulaRef<'a> {
    /// Convert the formula back to a flat list of symbols
    /// This is slow and shall not normally be called except for showing a result to the user.
    #[must_use]
    pub(crate) fn iter(self) -> Flatten<'a> {
        let mut f = Flatten {
            formula: self.formula,
            stack: vec![],
            sset: self.db.parse_result(),
            nset: self.db.name_result(),
        };
        f.step_into(self.root);
        f
    }

    /// Appends this formula to the provided stack buffer.
    ///
    /// The [`ProofBuilder`] structure uses a dense representation of formulas as byte strings,
    /// using the high bit to mark the end of each token.
    /// This function creates such a byte string, stores it in the provided buffer,
    /// and returns the range the newly added string occupies on the buffer.
    ///
    /// See [`crate::verify`] for more about this format.
    pub fn append_to_stack_buffer(self, stack_buffer: &mut Vec<u8>) -> Range<usize> {
        let tos = stack_buffer.len();
        let nset = &**self.db.name_result();
        for symbol in self {
            fast_extend(stack_buffer, nset.atom_name(symbol));
            *stack_buffer.last_mut().unwrap() |= 0x80;
        }
        let n_tos = stack_buffer.len();
        tos..n_tos
    }

    /// Builds the syntax proof for this formula.
    ///
    /// In Metamath, it is possible to write proofs that a given formula is a well-formed formula.
    /// This methos builds such a syntax proof for the formula into a [`crate::proof::ProofTree`],
    /// stores that proof tree in the provided [`ProofBuilder`] `arr`,
    /// and returns the index of that `ProofTree` within `arr`.
    pub fn build_syntax_proof<I: Copy, A: Default + FromIterator<I>>(
        self,
        stack_buffer: &mut Vec<u8>,
        arr: &mut dyn ProofBuilder<Item = I, Accum = A>,
    ) -> I {
        self.sub_build_syntax_proof(self.root, stack_buffer, arr)
    }

    /// Stores and returns the index of a [`ProofTree`] in a [`ProofBuilder`],
    /// corresponding to the syntax proof for the sub-formula with root at the given `node_id`.
    // Formulas children nodes are stored in the order of appearance of the variables
    // in the formula, which is efficient when parsing or rendering the formula from
    // or into a string of tokens. However, proofs require children nodes
    // sorted in the order of mandatory floating hypotheses.
    // This method performs this mapping.
    fn sub_build_syntax_proof<I: Copy, A: Default + FromIterator<I>>(
        self,
        node_id: NodeId,
        stack_buffer: &mut Vec<u8>,
        arr: &mut dyn ProofBuilder<Item = I, Accum = A>,
    ) -> I {
        let nset = self.db.name_result();

        let token = nset.atom_name(self.tree[node_id]);
        let address = nset.lookup_label(token).unwrap().address;
        let frame = self.db.scope_result().get(token).unwrap();
        let children_hyps = self
            .tree
            .children_iter(node_id)
            .map(|s_id| self.sub_build_syntax_proof(s_id, stack_buffer, arr))
            .collect::<Box<[I]>>();
        let hyps = frame
            .hypotheses
            .iter()
            .filter_map(|hyp| {
                if let Hyp::Floating(_sa, index, _) = hyp {
                    Some(children_hyps[*index])
                } else {
                    None
                }
            })
            .collect();
        let range = self.append_to_stack_buffer(stack_buffer);
        arr.build(address, hyps, stack_buffer, range)
    }
}

impl<'a> IntoIterator for FormulaRef<'a> {
    type Item = Symbol;
    type IntoIter = Flatten<'a>;
    fn into_iter(self) -> Flatten<'a> {
        self.iter()
    }
}

struct SubFormulaRef<'a> {
    node_id: NodeId,
    f_ref: FormulaRef<'a>,
}

impl<'a> Debug for SubFormulaRef<'a> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let label_name = as_str(
            self.f_ref
                .db
                .name_result()
                .atom_name(self.f_ref.formula.tree[self.node_id]),
        );
        let mut dt = f.debug_tuple(label_name);
        for s_id in self.f_ref.formula.tree.children_iter(self.node_id) {
            dt.field(&SubFormulaRef {
                node_id: s_id,
                f_ref: FormulaRef {
                    db: self.f_ref.db,
                    formula: self.f_ref.formula,
                },
            });
        }
        dt.finish()
    }
}

/// An iterator going through each symbol in a formula
#[derive(Debug)]
pub struct Flatten<'a> {
    formula: &'a Formula,
    stack: Vec<(TokenIter<'a>, Option<SiblingIter<'a, Label>>)>,
    sset: &'a SegmentSet,
    nset: &'a Nameset,
}

impl<'a> Flatten<'a> {
    fn step_into(&mut self, node_id: NodeId) {
        let label = self.formula.tree[node_id];
        let sref = self.sset.statement(
            self.nset
                .lookup_label(self.nset.atom_name(label))
                .unwrap()
                .address,
        );
        let mut math_iter = sref.math_iter();
        math_iter.next(); // Always skip the typecode token.
        if self.formula.tree.has_children(node_id) {
            self.stack
                .push((math_iter, Some(self.formula.tree.children_iter(node_id))));
        } else {
            self.stack.push((math_iter, None));
        };
    }
}

impl<'a> Iterator for Flatten<'a> {
    type Item = Symbol;

    fn next(&mut self) -> Option<Self::Item> {
        if self.stack.is_empty() {
            return None;
        }
        let stack_end = self.stack.len() - 1;
        let (ref mut math_iter, ref mut sibling_iter) = self.stack[stack_end];
        if let Some(token) = math_iter.next() {
            // Continue with next token of this syntax
            let symbol = self.nset.lookup_symbol(token.slice).unwrap();
            match (sibling_iter, symbol.stype) {
                (_, SymbolType::Constant) | (None, SymbolType::Variable) => Some(symbol.atom),
                (Some(ref mut iter), SymbolType::Variable) => {
                    // Variable : push into the next child
                    if let Some(next_child_id) = iter.next() {
                        self.step_into(next_child_id);
                        self.next()
                    } else {
                        panic!("Empty formula!");
                    }
                }
            }
        } else {
            // End of this formula, pop to the parent one
            self.stack.pop();
            self.next()
        }
    }

    // TODO(tirix): provide an implementation for size_hint?
}

impl<'a> Display for FormulaRef<'a> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let nset = &**self.db.name_result();
        write!(f, "{}", as_str(nset.atom_name(self.typecode)))?;
        for symbol in *self {
            write!(f, " {}", as_str(nset.atom_name(symbol)))?;
        }
        Ok(())
    }
}

impl<'a> Debug for FormulaRef<'a> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        SubFormulaRef {
            node_id: self.formula.root,
            f_ref: *self,
        }
        .fmt(f)
    }
}

#[derive(Default)]
pub(crate) struct FormulaBuilder {
    stack: Vec<NodeId>,
    variables: Bitset,
    tree: Tree<Label>,
}

/// A utility to build a formula.
impl FormulaBuilder {
    /// Every REDUCE pops `var_count` subformula items on the stack,
    /// and pushes one single new item, with the popped subformulas as children
    pub(crate) fn reduce(&mut self, label: Label, var_count: u8, offset: u8, is_variable: bool) {
        assert!(self.stack.len() >= (var_count + offset).into());
        let reduce_start = self.stack.len().saturating_sub((var_count + offset).into());
        let reduce_end = self.stack.len().saturating_sub(offset.into());
        let new_node_id = {
            let children = self.stack.drain(reduce_start..reduce_end);
            self.tree.add_node(label, children.as_slice())
        };
        if is_variable {
            self.variables.set_bit(new_node_id);
        }
        self.stack.insert(reduce_start, new_node_id);
    }

    pub(crate) fn build(self, typecode: TypeCode) -> Formula {
        // Only one entry shall remain in the stack at the time of building, the formula root.
        assert!(
            self.stack.len() == 1,
            "Final formula building state does not have one root - {:?}",
            self.stack
        );
        Formula {
            typecode,
            tree: Arc::new(self.tree),
            root: self.stack[0],
            variables: self.variables,
        }
    }
}
