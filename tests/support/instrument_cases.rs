use super::*;
use rustymusicsheets::{
    document,
    instruments::{classify, extract, job::DetectionJob, Detection, DetectionOptions, Instrument},
};

fn write_pdf(path: &Path, objects: Vec<Vec<u8>>) {
    let mut data = b"%PDF-1.4\n".to_vec();
    let mut offsets = vec![0];
    for (index, object) in objects.iter().enumerate() {
        offsets.push(data.len());
        data.extend_from_slice(format!("{} 0 obj\n", index + 1).as_bytes());
        data.extend_from_slice(object);
        data.extend_from_slice(b"\nendobj\n");
    }
    let xref = data.len();
    data.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", offsets.len()).as_bytes());
    for offset in &offsets[1..] {
        data.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    data.extend_from_slice(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
            offsets.len()
        )
        .as_bytes(),
    );
    fs::write(path, data).unwrap();
}

fn label_pdf(path: &Path, labels: &[&str]) {
    let mut stream = String::new();
    for (index, label) in labels.iter().enumerate() {
        stream.push_str(&format!(
            "BT /F1 28 Tf 50 {} Td ({label}) Tj ET\n",
            740 - index * 45
        ));
    }
    write_pdf(path, vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 << /Type /Font /Subtype /Type1 /BaseFont /Helvetica >> >> >> /Contents 4 0 R >>".to_vec(),
        format!("<< /Length {} >>\nstream\n{stream}\nendstream", stream.len()).into_bytes(),
    ]);
}

fn scanned_pdf(path: &Path, image: &image::RgbaImage) {
    let rgb = image::DynamicImage::ImageRgba8(image.clone()).into_rgb8();
    let mut jpeg = std::io::Cursor::new(Vec::new());
    rgb.write_to(&mut jpeg, image::ImageFormat::Jpeg).unwrap();
    let jpeg = jpeg.into_inner();
    let stream = "q 612 0 0 792 0 0 cm /Scan Do Q";
    let mut image_object = format!(
        "<< /Type /XObject /Subtype /Image /Width {} /Height {} /ColorSpace /DeviceRGB /BitsPerComponent 8 /Filter /DCTDecode /Length {} >>\nstream\n",
        image.width(), image.height(), jpeg.len()).into_bytes();
    image_object.extend_from_slice(&jpeg);
    image_object.extend_from_slice(b"\nendstream");
    write_pdf(path, vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /XObject << /Scan 5 0 R >> >> /Contents 4 0 R >>".to_vec(),
        format!("<< /Length {} >>\nstream\n{stream}\nendstream", stream.len()).into_bytes(),
        image_object,
    ]);
}

fn detection(text: &str) -> Detection {
    let mut result = Detection::default();
    classify::text(text, 1, "PDF text", &mut result);
    result
}

#[test]
fn embedded_pdf_text_is_used_before_ocr() {
    let (temp, _) = setup();
    let path = temp.path().join("Text score.pdf");
    label_pdf(&path, &["Alto Saxophone 1", "Violoncello II", "Piano"]);
    let result = extract::detect(
        &path,
        1,
        &DetectionOptions::default(),
        &AtomicBool::new(false),
        |_| {},
    )
    .unwrap();
    assert!(result.error.is_none(), "{:?}", result.error);
    assert_eq!(result.candidates.len(), 3);
    assert!(result
        .candidates
        .iter()
        .all(|c| c.method == "PDF text" && c.clear));
    assert!(result
        .candidates
        .iter()
        .any(|c| c.instrument.name == "Cello" && c.instrument.part == "2"));
}

#[test]
fn real_tesseract_reads_scanned_pdf_png_and_jpeg() {
    let (temp, _) = setup();
    let text = temp.path().join("Source.pdf");
    label_pdf(&text, &["Cello", "Piano"]);
    let image = pdf::render(&text, 0, 1400, &AtomicBool::new(false)).unwrap();
    let png = temp.path().join("Score.PNG");
    let jpeg = temp.path().join("Score.JPG");
    let scan = temp.path().join("Scanned.pdf");
    image
        .save_with_format(&png, image::ImageFormat::Png)
        .unwrap();
    image::DynamicImage::ImageRgba8(image.clone())
        .into_rgb8()
        .save_with_format(&jpeg, image::ImageFormat::Jpeg)
        .unwrap();
    scanned_pdf(&scan, &image);
    for path in [&png, &jpeg, &scan] {
        let result = extract::detect(
            path,
            1,
            &DetectionOptions::default(),
            &AtomicBool::new(false),
            |_| {},
        )
        .unwrap();
        assert!(
            result.error.is_none(),
            "{}: {:?}",
            path.display(),
            result.error
        );
        for name in ["Cello", "Piano"] {
            assert!(
                result.candidates.iter().any(|c| c.instrument.name == name
                    && c.method.starts_with("Tesseract")
                    && c.clear),
                "{}: {:?}",
                path.display(),
                result
            );
        }
    }
}

#[test]
fn image_import_render_watch_and_folder_copy_preserve_format() {
    use rustymusicsheets::{folder, watch::Scanner};
    let (temp, paths) = setup();
    let source = temp.path().join("images");
    fs::create_dir(&source).unwrap();
    let image = image::RgbaImage::from_pixel(40, 60, image::Rgba([0, 0, 0, 0]));
    image.save(source.join("Transparent.PNG")).unwrap();
    image::DynamicImage::ImageRgba8(image)
        .into_rgb8()
        .save(source.join("Photo.jpeg"))
        .unwrap();
    let report = batch(
        std::slice::from_ref(&source),
        true,
        &paths,
        &AtomicBool::new(false),
        |_| {},
    )
    .unwrap();
    assert_eq!(report.imported, 2);
    let db = Library::open(&paths).unwrap();
    for score in db.scores().unwrap() {
        assert_eq!(score.pages, 1);
        let path = db.pdf_path(&paths, &score.hash).unwrap();
        assert_ne!(path.extension().unwrap(), "pdf");
        let rendered = document::render(&path, 0, 600, &AtomicBool::new(false)).unwrap();
        assert!(rendered.width() > 0);
        assert!(document::render(&path, 1, 600, &AtomicBool::new(false)).is_err());
        if score.title == "Transparent" {
            assert_eq!(rendered.get_pixel(0, 0).0, [255, 255, 255, 255]);
        }
    }
    let moved = folder::select(
        &paths,
        &temp.path().join("New music"),
        &AtomicBool::new(false),
        |_| {},
    )
    .unwrap();
    for score in db.scores().unwrap() {
        assert!(db.pdf_path(&moved, &score.hash).unwrap().exists());
    }
    let new_image = moved.score_dir().join("Arrival.jpg");
    image::RgbImage::from_pixel(30, 50, image::Rgb([20, 40, 60]))
        .save(&new_image)
        .unwrap();
    let mut scanner = Scanner::default();
    let report = scanner
        .scan(
            &moved,
            &AtomicBool::new(false),
            Instant::now(),
            Duration::ZERO,
        )
        .unwrap();
    assert_eq!(report.imported, 1);
}

#[test]
fn manual_choices_and_manual_empty_values_survive_repeated_detection() {
    let (_temp, paths) = setup();
    let db = Library::open(&paths).unwrap();
    let id = db.insert("hash", "Part", 1, "").unwrap();
    assert!(db
        .store_detection(id, "hash", &detection("Cello\nPiano"), true)
        .unwrap());
    let mut score = db.scores().unwrap().remove(0);
    assert_eq!(score.instruments.len(), 2);
    assert!(!score.instruments_manual);
    score.instruments = vec![Instrument {
        name: "Sax".into(),
        variant: "Alto".into(),
        part: "1".into(),
    }];
    db.save_score(&score).unwrap();
    assert!(!db
        .store_detection(id, "hash", &detection("Cello"), true)
        .unwrap());
    let mut score = db.scores().unwrap().remove(0);
    assert!(score.instruments_manual);
    assert_eq!(score.instruments[0].name, "Saxophone");
    assert!(score.matches("Alto Saxophone"));
    score.instruments.clear();
    db.save_score(&score).unwrap();
    assert!(!db
        .store_detection(id, "hash", &detection("Cello"), true)
        .unwrap());
    let mut score = db.scores().unwrap().remove(0);
    assert!(score.instruments.is_empty() && score.instruments_manual);
    score.instruments_manual = false;
    db.save_score(&score).unwrap();
    assert!(db
        .store_detection(id, "hash", &detection("Cello"), true)
        .unwrap());
}

#[test]
fn stale_results_and_deleted_scores_cannot_be_applied() {
    let (_temp, paths) = setup();
    let db = Library::open(&paths).unwrap();
    let id = db
        .insert_file("old", "Part", 1, "", Path::new("Part.pdf"))
        .unwrap();
    db.store_detection(id, "old", &detection("Cello"), false)
        .unwrap();
    db.insert_file("new", "Part", 1, "", Path::new("Part.pdf"))
        .unwrap();
    assert!(db
        .store_detection(id, "old", &detection("Piano"), true)
        .is_err());
    assert!(db
        .apply_instruments(
            id,
            "old",
            &[Instrument {
                name: "Piano".into(),
                ..Default::default()
            }]
        )
        .is_err());
    assert!(db.detection(id, "old").unwrap().is_none());
    assert!(db.detection(id, "new").unwrap().is_none());
    assert!(db.instruments_for(id).unwrap().is_empty());
    let connection = rusqlite::Connection::open(paths.data.join("library.sqlite3")).unwrap();
    connection
        .pragma_update(None, "foreign_keys", "ON")
        .unwrap();
    connection
        .execute("DELETE FROM scores WHERE id=?1", [id])
        .unwrap();
    assert!(db
        .store_detection(id, "new", &detection("Cello"), false)
        .is_err());
    assert!(db.apply_instruments(id, "new", &[]).is_err());
}

#[test]
fn version_two_migration_backs_up_and_preserves_existing_library() {
    let (_temp, paths) = setup();
    let db = Library::open(&paths).unwrap();
    let id = db.insert("old", "Existing title", 3, "Bach").unwrap();
    db.remember_page(id, 2).unwrap();
    let group = db.create_group("Concert", "setlist").unwrap();
    db.add_entry(group, id).unwrap();
    drop(db);
    let connection = rusqlite::Connection::open(paths.data.join("library.sqlite3")).unwrap();
    connection
        .execute_batch(
            "DROP TABLE score_instruments; DROP TABLE instrument_detections;
        ALTER TABLE scores DROP COLUMN instruments_manual; PRAGMA user_version=2;",
        )
        .unwrap();
    drop(connection);
    let db = Library::open(&paths).unwrap();
    let score = db.scores().unwrap().remove(0);
    assert_eq!(score.title, "Existing title");
    assert_eq!(score.last_page, 2);
    assert!(score.instruments.is_empty() && !score.instruments_manual);
    assert_eq!(db.entries(group).unwrap()[0].score_id, id);
    let backup = fs::read_dir(paths.data.join("backups"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let snapshot = rusqlite::Connection::open(backup).unwrap();
    assert_eq!(
        snapshot
            .pragma_query_value(None, "user_version", |r| r.get::<_, u32>(0))
            .unwrap(),
        2
    );
}

#[test]
fn detection_job_stores_proposals_without_changing_manual_metadata() {
    let (temp, paths) = setup();
    let path = temp.path().join("Cello.pdf");
    label_pdf(&path, &["Violoncello II"]);
    let db = Library::open(&paths).unwrap();
    import_file(&path, &paths, &db, &AtomicBool::new(false)).unwrap();
    let score = db.scores().unwrap().remove(0);
    let job = DetectionJob::start(
        paths.clone(),
        vec![score.clone()],
        DetectionOptions::default(),
        false,
        || {},
    );
    loop {
        let update = job.updates.recv_timeout(Duration::from_secs(15)).unwrap();
        if update.done {
            assert_eq!(update.summary.failed, 0);
            break;
        }
    }
    let result = db.detection(score.id, &score.hash).unwrap().unwrap();
    assert_eq!(result.candidates[0].instrument.name, "Cello");
    assert!(db.instruments_for(score.id).unwrap().is_empty());
    assert_eq!(db.scores().unwrap()[0].detection_status, "review");
}

#[test]
fn invalid_language_malformed_images_and_cancellation_are_reported() {
    let (temp, _) = setup();
    let path = temp.path().join("Part.pdf");
    label_pdf(&path, &["Cello"]);
    let options = DetectionOptions {
        language: "eng;bad".into(),
        ..Default::default()
    };
    assert!(extract::detect(&path, 1, &options, &AtomicBool::new(false), |_| {}).is_err());
    assert!(extract::detect(
        &path,
        1,
        &DetectionOptions::default(),
        &AtomicBool::new(true),
        |_| {}
    )
    .is_err());
    let image = temp.path().join("Broken.png");
    fs::write(&image, b"not an image").unwrap();
    assert!(document::read_image(&image).is_err());
    let mut pixels = image::RgbImage::from_pixel(60, 60, image::Rgb([255, 255, 255]));
    pixels.put_pixel(10, 10, image::Rgb([0, 0, 0]));
    pixels.save(&image).unwrap();
    let missing = DetectionOptions {
        language: "missing_rustymusicsheets_language".into(),
        ..Default::default()
    };
    let result = extract::detect(&image, 1, &missing, &AtomicBool::new(false), |_| {}).unwrap();
    assert!(result.error.is_some());
}

#[test]
fn missing_tesseract_is_reported_by_the_packaged_command_path() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("Part.png");
    let mut pixels = image::RgbImage::from_pixel(100, 120, image::Rgb([255, 255, 255]));
    pixels.put_pixel(10, 10, image::Rgb([0, 0, 0]));
    pixels.save(&source).unwrap();
    let command = |argument: &str| {
        let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_rustymusicsheets"));
        command
            .arg(argument)
            .env("XDG_DATA_HOME", root.path().join("data"))
            .env("XDG_CONFIG_HOME", root.path().join("config"))
            .env("XDG_CACHE_HOME", root.path().join("cache"))
            .env("PATH", "");
        command
    };
    assert!(command("--import")
        .arg(&source)
        .output()
        .unwrap()
        .status
        .success());
    let result = command("--detect-instruments").output().unwrap();
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("tesseract"));
}

#[test]
fn cancelling_a_running_detection_does_not_store_partial_results() {
    let (temp, paths) = setup();
    let source = temp.path().join("Part.pdf");
    label_pdf(&source, &["Cello"]);
    let db = Library::open(&paths).unwrap();
    import_file(&source, &paths, &db, &AtomicBool::new(false)).unwrap();
    let score = db.scores().unwrap().remove(0);
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
    let worker_barrier = barrier.clone();
    let first = AtomicBool::new(true);
    let job = DetectionJob::start(
        paths,
        vec![score.clone()],
        DetectionOptions::default(),
        true,
        move || {
            if first.swap(false, Ordering::SeqCst) {
                worker_barrier.wait();
            }
        },
    );
    let initial = job.updates.recv_timeout(Duration::from_secs(5)).unwrap();
    assert!(!initial.done);
    job.cancel();
    barrier.wait();
    loop {
        let update = job.updates.recv_timeout(Duration::from_secs(5)).unwrap();
        if update.done {
            assert!(update.summary.cancelled);
            break;
        }
    }
    assert!(db.detection(score.id, &score.hash).unwrap().is_none());
    assert!(db.instruments_for(score.id).unwrap().is_empty());
}

#[test]
fn huge_png_dimensions_are_rejected_before_pixel_allocation() {
    let (temp, _) = setup();
    fn chunk(output: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
        output.extend_from_slice(&(data.len() as u32).to_be_bytes());
        output.extend_from_slice(kind);
        output.extend_from_slice(data);
        let mut crc = !0u32;
        for byte in kind.iter().chain(data) {
            crc ^= u32::from(*byte);
            for _ in 0..8 {
                crc = (crc >> 1) ^ (0xedb8_8320u32 & (0u32.wrapping_sub(crc & 1)));
            }
        }
        output.extend_from_slice(&(!crc).to_be_bytes());
    }
    let mut data = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut header = Vec::new();
    header.extend_from_slice(&10_000u32.to_be_bytes());
    header.extend_from_slice(&5_000u32.to_be_bytes());
    header.extend_from_slice(&[8, 2, 0, 0, 0]);
    chunk(&mut data, b"IHDR", &header);
    chunk(
        &mut data,
        b"IDAT",
        &[0x78, 0x9c, 0x03, 0x00, 0x00, 0x00, 0x00, 0x01],
    );
    chunk(&mut data, b"IEND", &[]);
    let path = temp.path().join("Huge.png");
    fs::write(&path, data).unwrap();
    assert!(document::read_image(&path).is_err());
}

#[test]
fn uniform_regions_do_not_invoke_tesseract() {
    let (temp, _) = setup();
    let path = temp.path().join("Blank.png");
    image::RgbImage::from_pixel(100, 140, image::Rgb([255, 255, 255]))
        .save(&path)
        .unwrap();
    let result = extract::detect(
        &path,
        1,
        &DetectionOptions {
            language: "missing_rustymusicsheets_language".into(),
            ..Default::default()
        },
        &AtomicBool::new(false),
        |_| {},
    )
    .unwrap();
    assert_eq!(result.pages, 1);
    assert!(result.candidates.is_empty());
    assert!(result.error.is_none());
}
