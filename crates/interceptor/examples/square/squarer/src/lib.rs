wit_bindgen::generate!({
    path: "../../wit",
    world: "advice-world",
    generate_all
});

use exports::modulewise::interceptor::advice::{
    AfterAction, Arg, BeforeAction, Guest, GuestInvocation, Value,
};

struct Squarer;

impl Guest for Squarer {
    type Invocation = SquareInvocation;
}

struct SquareInvocation {
    args: Vec<Arg>,
}

impl GuestInvocation for SquareInvocation {
    fn new(_function_name: String, args: Vec<Arg>) -> Self {
        Self { args }
    }

    fn before(&self) -> BeforeAction {
        BeforeAction::Proceed(self.args.clone())
    }

    fn after(&self, ret: Option<Value>) -> AfterAction {
        match ret {
            Some(Value::NumS64(n)) => AfterAction::Accept(Some(Value::NumS64(n * n))),
            other => AfterAction::Accept(other),
        }
    }
}

export!(Squarer);
