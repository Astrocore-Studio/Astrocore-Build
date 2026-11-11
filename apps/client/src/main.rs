use astrocore_client::{USAGE, build_app, parse_args};
use client::BuildClientPlugin;

fn main() {
    let raw: Vec<String> = std::env::args().skip(1).collect();
    let autoplay = raw.iter().any(|a| a == "--autoplay");
    let raw_third_person = raw.iter().any(|a| a == "--third-person");
    let mut args = match parse_args(
        raw.into_iter()
            .filter(|a| a != "--autoplay" && a != "--third-person"),
    ) {
        Ok(a) => a,
        Err(msg) => {
            if !msg.is_empty() {
                eprintln!("ошибка: {msg}\n");
            }
            eprint!("{USAGE}");
            std::process::exit(if msg.is_empty() { 0 } else { 2 });
        }
    };
    // Билд играется от первого лица; V возвращает вид сзади.
    args.first_person = !raw_third_person;
    // Клавиша E в клиенте движка отправляет команду взаимодействия билда.
    args.interact_command
        .get_or_insert_with(|| "interact".into());
    let manifest = args.render_manifest.clone();
    let mut app = build_app(args);
    app.add_plugins(BuildClientPlugin { manifest, autoplay });
    app.run();
}
