use astrocore_client::{USAGE, build_app, parse_args};
use client::BuildClientPlugin;

fn main() {
    let raw: Vec<String> = std::env::args().skip(1).collect();
    let autoplay = raw.iter().any(|a| a == "--autoplay");
    let mut args = match parse_args(raw.into_iter().filter(|a| a != "--autoplay")) {
        Ok(a) => a,
        Err(msg) => {
            if !msg.is_empty() {
                eprintln!("ошибка: {msg}\n");
            }
            eprint!("{USAGE}");
            std::process::exit(if msg.is_empty() { 0 } else { 2 });
        }
    };
    // Клавиша E в клиенте движка отправляет команду взаимодействия билда.
    args.interact_command
        .get_or_insert_with(|| "interact".into());
    let manifest = args.render_manifest.clone();
    let mut app = build_app(args);
    app.add_plugins(BuildClientPlugin { manifest, autoplay });
    app.run();
}
