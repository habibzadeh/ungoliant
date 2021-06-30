use std::{collections::HashMap, io::Write, path::PathBuf};

use crate::classify::Classifier;
use crate::error::Error;
use crate::lang::LangFiles;
use crate::lang::LANG;
use crate::pipeline::pipeline::Pipeline;
use crate::shard::wet::Wet;
use itertools::Itertools;
use log::{debug, info, warn};
use rayon::prelude::*;
use std::hash::BuildHasherDefault;
use twox_hash::XxHash64;
use warc::RawRecord;
pub struct RayonShard {
    src: PathBuf,
    dst: PathBuf,
    nb_shards: Option<usize>,
    nb_records: Option<usize>,
}

/// container for (lang, sentences) pairs
#[derive(Debug)]
struct ShardContent {
    pub inner: HashMap<&'static str, Vec<String>, BuildHasherDefault<XxHash64>>,
}

impl ShardContent {
    /// create a new, empty [ShardContent]. Uses [Default::default] for initialization
    pub fn new() -> Self {
        ShardContent {
            inner: Default::default(),
        }
    }

    /// inserts `sentence` into `lang` vector
    ///
    /// Creates `lang` vector if non existent
    pub fn insert(&mut self, sentence: String, lang: &'static str) {
        if let Some(sentences) = self.inner.get_mut(&lang) {
            sentences.push(sentence)
        } else {
            let mut sentences = Vec::new();
            let ret = sentences.push(sentence);
            self.inner.insert(lang, sentences);
            ret
        }
    }
}

/// Processing pipeline.
///
impl RayonShard {
    /// create a new pipeline
    /// concurrenct on shards **only**
    ///
    /// - `nb_shards` limits the number of shards that will be processed
    /// - `nb_records` limites the number of records per shard that will be processed
    #[allow(dead_code)]
    pub fn new(
        src: PathBuf,
        dst: PathBuf,
        nb_shards: Option<usize>,
        nb_records: Option<usize>,
    ) -> Self {
        Self {
            src,
            dst,
            nb_shards,
            nb_records,
        }
    }

    /// Process a provided record.
    fn process_record(record: RawRecord, cls: &Classifier) -> Option<Vec<(String, &'static str)>> {
        let body = String::from_utf8(record.body).ok();

        // process record if body is utf8-valid
        if let Some(sentences) = body {
            // filter out lines that does not contain 100 characters.
            let sentences = sentences.lines().filter(|line| line.chars().count() > 100);

            let results: Vec<(String, &'static str)> = sentences
                // predict for each sentence, discarding
                // predictions that does not meet threshold
                .filter_map(|sentence| {
                    let prediction = cls.predict(&sentence).ok();
                    // let prediction = cls.predict(sentence).ok();

                    if let Some(Some(lang)) = prediction {
                        //TODO: rewrite these two lines more elegantly
                        //      we can unwrap since predict returns None if no predictions are
                        //      found
                        let lang = lang.get(0).unwrap();

                        // check if fasttext provided lang exists
                        // return None if not
                        match LANG.get(lang.label.as_str()) {
                            Some(lang) => Some((sentence.to_string(), *lang)),
                            None => {
                                warn!("lang {} does not exist!", lang.label);
                                None
                            }
                        }
                    } else {
                        None
                    }
                })
                .collect();

            Some(results)
        } else {
            None
        }
    }
}

impl Pipeline<()> for RayonShard {
    fn run(&self) -> Result<(), Error> {
        let cls = Classifier::new_lid()?;

        // list files in source folder,
        // filter out errors from fs and from gzip/wet.
        // This means that invalid gz files and invalid
        // wet files are discarded silently
        let results = std::fs::read_dir(&self.src)?
            //TODO: log errors!
            //      using ok() silently discards errors
            //      use inspect to log?
            .filter_map(|shard| shard.ok())
            .filter_map(|shard| Wet::from_path_gzip(&shard.path()).ok());

        // if let Some(nb_shards) = self.nb_shards {
        //     let results = results.take(nb_shards);
        // }

        // convert to parallel iterator
        // /!\: We use par_bridge, that is suboptimal
        //      compared to implementing IntoParallelIterator
        //      ourselves.
        let results = results.enumerate().par_bridge();

        // holds file handles
        let langfiles = LangFiles::new(&self.dst)?;

        // iterate over shards
        results.for_each(|(idx, shard)| {
            let mut sorted_sentences = ShardContent::new();
            info!("processing shard {:?}", idx);

            // convert into an iterator
            let wetfile = shard.enumerate();

            let shard_results: Vec<Vec<(String, &'static str)>> = wetfile
                .filter_map(|(idx_record, record)| match record {
                    Ok(record) => RayonShard::process_record(record, &cls),
                    Err(e) => {
                        warn!("Error on record {} of shard {}: {}", idx_record, idx, e);
                        None
                    }
                })
                .collect(); //TODO: test with a for_each and a channel to send?

            // store predictions into sorted_sentences
            for record in shard_results {
                record
                    .into_iter()
                    .for_each(|(sentence, lang)| sorted_sentences.insert(sentence, lang));
            }

            // write to disk
            debug!("writing shard {:?} into lang files", idx);
            for (lang, sentences) in sorted_sentences.inner {
                let mut fd = langfiles.get(&lang).unwrap();
                let content = sentences.into_iter().join("\n");
                fd.write_all(&content.as_bytes()).unwrap();
            }
        });

        Ok(())
    }
}
