use std::ffi::OsString;
use std::fs::{self, File};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

fn valid(path: &Path) -> bool {
    path.extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("exe"))
        && path.is_file()
}

fn copy_output(stream: impl Read, mut console: impl Write, log: &Mutex<File>) -> io::Result<()> {
    let mut stream = BufReader::new(stream);
    let mut line = Vec::new();
    while stream.read_until(b'\n', &mut line)? != 0 {
        console.write_all(&line)?;
        log.lock()
            .map_err(|_| io::Error::other("registro bloqueado"))?
            .write_all(&line)?;
        line.clear();
    }
    Ok(())
}

fn run() -> Result<i32, Box<dyn std::error::Error>> {
    let root = std::env::current_exe()?
        .parent()
        .ok_or("Carpeta del lanzador no disponible")?
        .to_owned();
    let (mut diagnostic, mut version, mut change, mut no_save) = (false, false, false, false);
    let mut title = None;
    for arg in std::env::args_os().skip(1) {
        match arg.to_str() {
            Some("--diagnostico") => diagnostic = true,
            Some("--version") => version = true,
            Some("--elegir") => change = true,
            Some("--no-recordar") => no_save = true,
            Some("--ayuda" | "--help") => {
                println!("Iniciar.exe [--diagnostico] [--elegir | RUTA_EXE] [--version]");
                return Ok(0);
            }
            _ if title.is_none() => title = Some(PathBuf::from(arg)),
            _ => return Err("Indica una sola ruta de ejecutable".into()),
        }
    }
    let binary = root.join("Actual").join(if diagnostic {
        "VIctory-Diagnostico.exe"
    } else {
        "VIctory.exe"
    });
    if version {
        return Ok(Command::new(binary)
            .arg("--version")
            .status()?
            .code()
            .unwrap_or(1));
    }
    let config_dir = root.join("Configuracion");
    let config = config_dir.join("Programa-local.txt");
    // A path saved by an earlier layout is still honoured; the next save
    // writes it under Configuracion without deleting the old copy.
    let legacy = root.join("Programa-local.txt");
    let previous = if change {
        None
    } else {
        fs::read_to_string(&config)
            .or_else(|_| fs::read_to_string(&legacy))
            .ok()
            .map(|text| PathBuf::from(text.trim_end_matches(['\r', '\n'])))
            .filter(|path| valid(path))
    };
    let remember = !no_save && (title.is_some() || previous.is_none());
    let target = match title.or(previous) {
        Some(path) => path,
        None => {
            print!("Ruta completa del programa .exe autorizado (Enter para cancelar): ");
            io::stdout().flush()?;
            let mut input = String::new();
            io::stdin().read_line(&mut input)?;
            let input = input.trim().trim_matches('"');
            if input.is_empty() {
                return Ok(0);
            }
            PathBuf::from(input)
        }
    };
    if !valid(&target) {
        return Err(format!("No existe un archivo .exe en {}", target.display()).into());
    }
    let target = target.canonicalize()?;
    // The Win32 local extended-path prefix is valid but the guest maps DOS paths.
    let target = target
        .to_str()
        .ok_or("La ruta no se puede guardar como texto")?;
    let target = target
        .strip_prefix(r"\\?\")
        .filter(|s| s.as_bytes().get(1) == Some(&b':'))
        .unwrap_or(target);
    let mut command = Command::new(binary);
    command
        .current_dir(&root)
        .env("VICTORY_DISPLAY_MODE", "1920x1080")
        .args([
            OsString::from("run"),
            OsString::from(target),
            OsString::from("--execute"),
        ]);
    if diagnostic {
        command.env("VICTORY_D3D12_DEBUG", "1");
    }
    let stamp = SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis();
    let logs = root.join("Logs");
    fs::create_dir_all(&logs)?;
    let log = Arc::new(Mutex::new(File::create_new(
        logs.join(format!("{stamp}-{}.log", std::process::id())),
    )?));
    let mut child = command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let stdout = child.stdout.take().ok_or("Salida no disponible")?;
    let stderr = child
        .stderr
        .take()
        .ok_or("Salida de errores no disponible")?;
    let out_log = log.clone();
    let out = std::thread::spawn(move || copy_output(stdout, io::stdout().lock(), &out_log));
    let err = std::thread::spawn(move || copy_output(stderr, io::stderr().lock(), &log));
    let status = child.wait()?;
    for stream in [out, err] {
        stream
            .join()
            .map_err(|_| "No se pudo capturar la salida")??;
    }
    if remember && status.success() {
        fs::create_dir_all(&config_dir)?;
        fs::write(config, target)?;
    }
    Ok(status.code().unwrap_or(1))
}

fn main() -> ExitCode {
    match run() {
        Ok(code) => std::process::exit(code),
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}
