use std::{env, path::PathBuf};

use pdf::file::FileOptions;
use pdf_tools::page_text;

fn main() {
    let path = PathBuf::from(env::args_os().nth(1).expect("no file given"));
    let file = FileOptions::cached().open(&path).unwrap();

    for (_page_nr, page) in file.pages().enumerate() {
        if let Ok(page) = page {
            if let Ok(text) = page_text(&page, &file) {
                print!("{}", text);
            } else {
                println!("ERROR");
            }
        }
    }
}
