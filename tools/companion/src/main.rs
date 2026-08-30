use companion::{Config, run};
use std::env;
use std::io;

fn main() -> io::Result<()> {
    let config = Config::from_args(env::args())?;
    run(config)
}
