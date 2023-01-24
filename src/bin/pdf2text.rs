use pdf::file::File;
use pdf_tools::page_text;

fn main() {
    let input = std::env::args().nth(1).expect("no input file given");
    let file = File::open(input).expect("failed to read PDF");
    for (page_nr, page) in file.pages().enumerate() {
        if let Ok(page) = page {
            println!("=== PAGE {} ===\n", page_nr);
            if let Ok(text) = page_text(&page, &file) {
                println!("{}", text);
            } else {
                println!("ERROR");
            }
            println!();
        }
    }
}
