# Named Imports Example

A greeter that imports the same `translator` interface twice, once as
`spanish` and once as `french`.

## Structure

```
named-imports/
├── greeter/          # Imports two translators, exports greet(name)
├── translator/       # Translates words using its config
├── wit/
│   └── package.wit   # The translator interface and both worlds
└── config.toml       # One greeter and two translators
```

The greeter's world gives each import its own name:

```wit
world greeter {
    import spanish: translator;
    import french: translator;

    export greet: func(name: string) -> list<string>;
}
```

A named import is satisfied by the component with that name. Here both are the
same `translator.wasm` with different config:

```toml
[component.greeter]
uri = "./lib/greeter.wasm"
imports = ["spanish", "french"]

[component.spanish]
uri = "./lib/translator.wasm"
config.hello = "hola"
config.world = "mundo"

[component.french]
uri = "./lib/translator.wasm"
config.hello = "bonjour"
config.world = "le monde"
```

## Build

```bash
./build.sh
```

## Run

```bash
./run.sh
```

Output:
```
[
  "hello world!",
  "hola mundo!",
  "bonjour le monde!"
]
```
