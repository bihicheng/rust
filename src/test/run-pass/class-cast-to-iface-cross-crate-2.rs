// xfail-fast
// aux-build:cci_class_cast.rs
use cci_class_cast;
import cci_class_cast::kitty::*;
import to_str::*;
import to_str::to_str;

fn print_out<T: to_str>(thing: T, expected: str) {
  let actual = thing.to_str();
  #debug("%s", actual);
  assert(actual == expected);
}

fn main() {
  let nyan : to_str  = cat(0u, 2, "nyan") as to_str;
  print_out(nyan, "nyan");
}

