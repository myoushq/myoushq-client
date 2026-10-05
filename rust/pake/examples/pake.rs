//! Command-line PAKE, for interop checks against other implementations:
//!   pake start  a|b CODE SEED_HEX            -> our message (hex)
//!   pake finish a|b CODE SEED_HEX PEER_HEX   -> shared key (hex)

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn unhex(s: &str) -> Vec<u8> {
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).expect("hex")).collect()
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let role = myous_pake::Role::parse(&args[2]).expect("role a or b");
    let seed: [u8; 32] = unhex(&args[4]).try_into().expect("32-byte seed");
    match args[1].as_str() {
        "start" => println!("{}", hex(&myous_pake::start(role, &args[3], seed))),
        "finish" => match myous_pake::finish(role, &args[3], seed, &unhex(&args[5])) {
            Ok(key) => println!("{}", hex(&key)),
            Err(e) => {
                eprintln!("{e}");
                std::process::exit(1);
            }
        },
        _ => panic!("start or finish"),
    }
}
