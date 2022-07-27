/*! Thread-safe language-separated text/metadata writer.

Each language (provided by [crate::lang::LANG]) is given a [self::Writer] wrapped into an [Arc<Mutex<Writer>>].

## Warning

!*/
use std::{
    collections::HashMap,
    marker::PhantomData,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, RwLock},
};

use log::info;
use oxilangtag::LanguageTag;

use crate::lang::LANG;
use crate::{error, identifiers::model::ModelKind};
use crate::{error::Error, io::writer::Writer};

use super::writer::{WriterDoc, WriterTrait};
/// Holds references to [Writer].
pub struct LangFiles {
    writers: HashMap<&'static str, Arc<Mutex<Writer>>>,
}

type LanguageMap = HashMap<LanguageTag<String>, Arc<Mutex<WriterDoc>>>;
pub struct LangFilesDoc<T: ModelKind> {
    writers: Arc<RwLock<LanguageMap>>,
    kind: PhantomData<T>,
    dst: PathBuf,
    part_size_bytes: Option<u64>,
}

impl LangFiles {
    /// Create a new LangFiles. `part_size_bytes` sets an indication of the maximum size
    /// by part.
    /// Note that if it is set too low and a unique record can't be stored in an unique part
    /// then a part will still be created, being larger than the `part_size_bytes`. This is expected behaviour.
    ///
    /// Also keep in mind that [Self::close_meta] has to be called once every write is done.
    ///
    // [Self::close_meta] could be integrated in an `impl Drop`
    pub fn new(dst: &Path, part_size_bytes: Option<u64>) -> Result<Self, error::Error> {
        let mut writers = HashMap::with_capacity(LANG.len());
        let mut w;
        for lang in LANG.iter() {
            w = Writer::new(dst, lang, part_size_bytes)?;
            writers.insert(*lang, Arc::new(Mutex::new(w)));
        }

        Ok(Self { writers })
    }

    /// Get a non-mutable reference to the writers.
    pub fn writers(&self) -> &HashMap<&'static str, Arc<Mutex<Writer>>> {
        &self.writers
    }

    /// Fix open metadata files by removing trailing comma and closing the array.
    pub fn close_meta(&self) -> Result<(), error::Error> {
        for writer in self.writers.values() {
            let mut writer_lock = writer.lock().unwrap();
            writer_lock.close_meta()?;
        }
        Ok(())
    }
}

impl<T: ModelKind> LangFilesDoc<T> {
    /// Create a new LangFiles. `part_size_bytes` sets an indication of the maximum size
    /// by part.
    /// Note that if it is set too low and a unique record can't be stored in an unique part
    /// then a part will still be created, being larger than the `part_size_bytes`. This is expected behaviour.
    ///
    /// Also keep in mind that [Self::close_meta] has to be called once every write is done.
    ///
    // [Self::close_meta] could be integrated in an `impl Drop`
    pub fn new(dst: &Path, part_size_bytes: Option<u64>) -> Self {
        Self {
            writers: Arc::new(RwLock::new(HashMap::new())),
            kind: PhantomData,
            dst: dst.to_path_buf(),
            part_size_bytes,
        }
    }

    fn new_writer(
        dst: &Path,
        lang: LanguageTag<String>,
        part_size_bytes: Option<u64>,
    ) -> Result<Arc<Mutex<WriterDoc>>, Error> {
        //TODO: remove the box leak?
        // The idea is that when we encounter a new language we need to keep its
        // code alive for the rest of the process
        let lang: &'static str = Box::leak(lang.into_inner().into_boxed_str());
        let w = WriterDoc::new(dst, lang, part_size_bytes)?;

        Ok(Arc::new(Mutex::new(w)))
    }

    pub fn contains(&self, k: &LanguageTag<String>) -> bool {
        self.writers
            .read()
            .expect("Problem locking writers (in read)")
            .contains_key(k)
    }

    pub fn insert_writer(&self, k: LanguageTag<String>) -> Result<(), Error> {
        info!("Creating writer {k}");
        info!("{k}: Waiting for lock");
        let mut writer = self
            .writers
            .write()
            .expect("Problem with locking writers (in write)");

        // we use the entry API rather than insert to keep the
        // old writer if the lang already exists
        writer.entry(k.clone()).or_insert(Self::new_writer(
            &self.dst,
            k.clone(),
            self.part_size_bytes,
        )?);

        info!("{k}: Done");
        Ok(())
    }
    /// Get a non-mutable reference to the writers.
    // pub fn writers(&self) -> Arc<HashMap<LanguageTag<String>, Arc<Mutex<WriterDoc>>>> {
    pub fn writers(
        &self,
    ) -> std::sync::RwLockReadGuard<HashMap<LanguageTag<String>, Arc<Mutex<WriterDoc>>>> {
        self.writers.read().unwrap()
    }

    /// Fix open metadata files by removing trailing comma and closing the array.
    pub fn close_meta(&self) -> Result<(), error::Error> {
        let writers = self.writers.read().unwrap();
        for writer in writers.values() {
            let mut writer_lock = writer.lock().unwrap();
            writer_lock.close_meta()?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {

    use std::{fs::File, path::PathBuf};

    use crate::{
        identifiers::{identification::Identification, model::Old},
        pipelines::oscardoc::types::{Document, Metadata},
        pipelines::oscarmeta::types::MergedPiece,
    };
    use warc::{BufferedBody, Record, WarcHeader};

    use super::*;
    use tempfile::tempdir;

    type WarcHeaders = HashMap<WarcHeader, Vec<u8>>;

    fn create_merged_piece(
        sentences: String,
        identification: &'static str,
        headers: WarcHeaders,
    ) -> MergedPiece {
        let nb_sentences = sentences.split('\n').count();
        MergedPiece {
            sentences,
            identification,
            headers,
            nb_sentences,
        }
    }
    #[test]
    fn init() {
        let dst = Path::new("dst_langfiles_init");
        std::fs::create_dir(dst).unwrap();
        let _ = LangFiles::new(dst, Some(10));
        std::fs::remove_dir_all(dst).unwrap();
    }

    #[test]
    fn write_one() {
        let dst = Path::new("dst_langfiles_write_one");
        std::fs::create_dir(dst).unwrap();
        let langfiles = LangFiles::new(dst, Some(10)).unwrap();

        let sentences = "essai d'écriture
de trois lignes
hehe :)"
            .to_string();
        let headers = vec![(WarcHeader::ContentType, Vec::from("blogpost".as_bytes()))]
            .into_iter()
            .collect();
        let mp = vec![create_merged_piece(sentences, "fr", headers)];
        // lock mutex and acquire writer
        let fr_writer = langfiles.writers().get("fr").unwrap().clone();
        let mut fr_writer_locked = fr_writer.lock().unwrap();

        fr_writer_locked.write(mp).unwrap();
        std::fs::remove_dir_all(dst).unwrap();
    }

    #[test]
    fn init_doc() {
        let dst = tempdir().unwrap();
        let _: LangFilesDoc<Old> = LangFilesDoc::new(dst.path(), None);
    }

    #[test]
    fn write_one_doc() {
        let dst = tempdir().unwrap();
        let lf: LangFilesDoc<Old> = LangFilesDoc::new(dst.path(), None);

        let content = "Hello!".to_string();

        let record = Record::default();
        let record: Record<BufferedBody> = record.add_body(content);

        let record_id = Identification::new(LanguageTag::parse("en".to_string()).unwrap(), 1.0);
        let sentences_id = vec![Some(record_id.clone())];

        let metadata = Metadata::new(&record_id, &sentences_id);
        let (headers, content) = record.into_raw_parts();

        let docs = vec![Document::new(
            String::from_utf8_lossy(&content).to_string(),
            headers.headers,
            metadata,
        )];

        lf.insert_writer(docs[0].identification().label().clone())
            .unwrap();
        let w = lf
            .writers()
            .get(docs[0].identification().label())
            .unwrap()
            .clone();

        if let Ok(mut w) = w.try_lock() {
            w.write(docs.to_vec()).unwrap();
        }

        let mut read_path = PathBuf::from(dst.path());
        read_path.push("en_meta.jsonl");

        let b = File::open(read_path).unwrap();
        let doc_from_file: Document = serde_json::from_reader(b).unwrap();

        assert_eq!(doc_from_file, docs[0]);
    }
}
