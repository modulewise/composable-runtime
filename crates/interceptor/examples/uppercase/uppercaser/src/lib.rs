wit_bindgen::generate!({
    path: "../../wit",
    world: "advice-world",
    generate_all
});

use exports::modulewise::interceptor::advice::{
    AfterAction, Arg, BeforeAction, Guest, GuestInvocation, Value,
};

struct Uppercaser;

impl Guest for Uppercaser {
    type Invocation = UppercaseInvocation;
}

struct UppercaseInvocation {
    args: Vec<Arg>,
}

impl GuestInvocation for UppercaseInvocation {
    fn new(_function_name: String, args: Vec<Arg>) -> Self {
        Self { args }
    }

    fn before(&self) -> BeforeAction {
        BeforeAction::Proceed(self.args.clone())
    }

    fn after(&self, ret: Option<Value>) -> AfterAction {
        match ret {
            Some(Value::Str(s)) => AfterAction::Accept(Some(Value::Str(s.to_uppercase()))),
            other => AfterAction::Accept(other),
        }
    }
}

export!(Uppercaser);
