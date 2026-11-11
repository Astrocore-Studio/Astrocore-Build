use prototypes::PrototypeRegistry;

fn main() {
    let dir = std::env::args().nth(1).expect("каталог с прототипами SS14");
    let mut r = PrototypeRegistry::new();
    let n = r.load_dir(std::path::Path::new(&dir)).expect("загрузка");
    println!("прототипов entity: {n}, в реестре: {}", r.len());
    match r.validate() {
        Ok(()) => println!("наследование согласовано"),
        // Родители из других каталогов SS14 здесь не загружены — это ожидаемо.
        Err(e) => println!("валидация: {e}"),
    }
    if let Ok(c) = r.spawnable("Crowbar") {
        println!(
            "Crowbar: компонентов {} имя {:?}",
            c.components.len(),
            c.name
        );
    }
}
