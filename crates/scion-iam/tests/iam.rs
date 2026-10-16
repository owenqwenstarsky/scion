use scion_iam::{decide, narrow, Accounts, Action, AuthzError, PasswordParams, Role, Via};
use scion_store::Store;
use std::sync::Arc;

#[test]
fn matrix_matches_min_role() {
    for action in Action::all() {
        for role in [
            Role::None,
            Role::Read,
            Role::Write,
            Role::Maintain,
            Role::Admin,
            Role::Owner,
        ] {
            let result = decide(role, *action, false);
            if role == Role::None {
                assert_eq!(result, Err(AuthzError::NotFound));
            } else if role >= action.min_role() {
                assert!(result.is_ok(), "{role:?} {action:?}");
            } else {
                assert_eq!(result, Err(AuthzError::Forbidden));
            }
        }
        assert_eq!(
            decide(Role::Owner, *action, true),
            Err(AuthzError::Forbidden)
        );
    }
}

#[test]
fn token_cannot_widen() {
    let via = Via::Token {
        max_role: Role::Read,
        repos: None,
    };
    assert_eq!(narrow(Role::Owner, &via, 1), Role::Read);
    let via = Via::Token {
        max_role: Role::Admin,
        repos: Some(vec![2]),
    };
    assert_eq!(narrow(Role::Owner, &via, 1), Role::None);
    assert_eq!(narrow(Role::Write, &via, 2), Role::Write);
}

#[test]
fn bootstrap_once_and_visibility() {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(Store::open(dir.path()).unwrap());
    let accounts = Accounts::new(store, PasswordParams::fast_for_tests()).unwrap();
    assert!(accounts
        .create_first_admin("short", "password1234")
        .is_err());
    let admin = accounts
        .create_first_admin("AdaAdmin", "correct-horse-battery")
        .unwrap();
    assert!(accounts
        .create_first_admin("Other", "correct-horse-battery")
        .is_err());
    let login = accounts
        .login("AdaAdmin", "wrong-password-xx", "test")
        .unwrap_err();
    let _ = login;
    let session = accounts
        .login("AdaAdmin", "correct-horse-battery", "test")
        .unwrap();
    assert!(session.must_change_password);
    let principal = accounts
        .authenticate_session(&session.session_token)
        .unwrap()
        .unwrap();
    assert!(Accounts::csrf_matches(&principal, &session.csrf_token));
    assert!(!Accounts::csrf_matches(&principal, "nope"));

    let (bob, bob_pw) = accounts.create_user("bob", None, "human", false).unwrap();
    let bob_pw = bob_pw.unwrap();
    let repo = accounts.create_repo(admin, "app", "demo").unwrap();
    let bob_login = accounts.login("bob", &bob_pw, "test").unwrap();
    let bob_p = accounts
        .authenticate_session(&bob_login.session_token)
        .unwrap()
        .unwrap();
    assert_eq!(
        accounts.authorize(&bob_p, repo, Action::ObjectRead, None),
        Err(AuthzError::NotFound)
    );
    accounts.set_grant(repo, bob, Role::Read, admin).unwrap();
    assert!(accounts
        .authorize(&bob_p, repo, Action::ObjectRead, None)
        .is_ok());
    assert_eq!(
        accounts.authorize(&bob_p, repo, Action::Land, None),
        Err(AuthzError::Forbidden)
    );
    accounts
        .set_path_rule(repo, "secret/**", None, false, false, admin)
        .unwrap();
    assert_eq!(
        accounts.authorize(&bob_p, repo, Action::ObjectRead, Some("secret/key")),
        Err(AuthzError::Forbidden)
    );
    accounts.remove_grant(repo, bob).unwrap();
    assert!(accounts.list_repos(bob).unwrap().is_empty());
    assert_eq!(accounts.list_repos(admin).unwrap().len(), 1);

    // Lockout: five failures then still a generic error.
    for _ in 0..5 {
        let _ = accounts.login("bob", "not-the-password", "test");
    }
    assert!(accounts.login("bob", &bob_pw, "test").is_err());
    accounts.unlock(bob).unwrap();
    assert!(accounts.login("bob", &bob_pw, "test").is_ok());
}
