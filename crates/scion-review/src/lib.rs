//! A review can be approved by someone other than its author.
//! Persisting reviews and rendering them in the web UI is not implemented.

use thiserror::Error;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decision {
    Approve,
    RequestChanges,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Review {
    pub author: String,
    pub decision: Option<Decision>,
    pub reviewer: Option<String>,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum Error {
    #[error("the author cannot approve their own review")]
    SelfApproval,
}

impl Review {
    pub fn open(author: impl Into<String>) -> Self {
        Self {
            author: author.into(),
            decision: None,
            reviewer: None,
        }
    }

    pub fn decide(&mut self, reviewer: &str, decision: Decision) -> Result<(), Error> {
        if reviewer == self.author && decision == Decision::Approve {
            return Err(Error::SelfApproval);
        }
        self.reviewer = Some(reviewer.to_string());
        self.decision = Some(decision);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn author_cannot_self_approve() {
        let mut review = Review::open("ada");
        assert_eq!(
            review.decide("ada", Decision::Approve),
            Err(Error::SelfApproval)
        );
        review.decide("bea", Decision::Approve).unwrap();
        assert_eq!(review.decision, Some(Decision::Approve));
    }
}
