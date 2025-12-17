use std::fs::File;

use hintfile::Hints;

fn differentials(indexes: &[u64]) -> Vec<u64> {
    indexes
        .iter()
        .zip(indexes.iter().skip(1))
        .map(|(a, b)| b - a)
        .collect()
}

fn average(diffs: &[u64]) -> f64 {
    let sum = diffs.iter().sum::<u64>() as f64;
    if !diffs.is_empty() {
        return sum / diffs.len() as f64;
    }
    0.
}

fn main() {
    let path = std::env::args()
        .nth(1)
        .expect("Usage: </path/to/hintsfile>");
    let file = File::open(path).unwrap();
    let mut hints = Hints::from_file(file);
    let csv = File::create("./results.csv").unwrap();
    let mut wtr = csv::Writer::from_writer(csv);
    for height in 1..=hints.stop_height() {
        let unspents = hints.get_indexes(height);
        let max_unspent = unspents.iter().max().unwrap_or(&0);
        let diffs = differentials(&unspents);
        let average_diff = average(&diffs);
        println!("Block height: {height}; average diff: {average_diff}; max index: {max_unspent}; diffs: {diffs:?}");
        wtr.write_record(vec![
            height.to_string(),
            average_diff.to_string(),
            max_unspent.to_string(),
        ])
        .unwrap();
        // std::thread::sleep(Duration::from_millis(500));
    }
}
