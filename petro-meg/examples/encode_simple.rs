use std::fs::File;

use petro_meg::path::MegPath;
use petro_meg::version::MegV1;
use petro_meg::writer::BuildMeg as _;

fn main() {
    let mut builder = MegV1.builder();
    let path = MegPath::from_str("Some/Path/SomeFile.txt")
        .unwrap()
        .to_owned();
    builder.insert(path, "Hello World!".as_bytes());
    let mut file = File::create("encode-example.meg").expect("Unable to create output file");
    builder.build(&mut file).expect("Encoding failed");
}
