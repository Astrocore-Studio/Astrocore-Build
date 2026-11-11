use gameplay::items::item_info;
use prototypes::{Resolved, parse_ss14_yaml};

fn main() {
    let path = std::env::args().nth(1).expect("файл прототипов SS14");
    let text = std::fs::read_to_string(&path).expect("чтение");
    let (defs, report) = parse_ss14_yaml(&text).expect("разбор YAML");
    println!("{path}: прототипов entity {}, пропущено {}", report.entities, report.skipped_other_types);
    for d in defs {
        // Наследование здесь не разрешается (родители лежат в других каталогах SS14): смотрим
        // только собственные компоненты прототипа.
        let r = Resolved {
            id: d.id.clone(),
            name: d.name.clone(),
            components: d.components.clone(),
        };
        if let Some(i) = item_info(&r)
            && let Some(s) = &i.storage
        {
            println!(
                "  {}: размер {:?}, слоты {:?}, хранилище {} клеток, максимум {:?}, чёрный список {:?}",
                d.id,
                i.size,
                i.slots,
                s.cells.len(),
                s.max_item_size,
                s.blacklist.as_ref().map(|f| &f.tags)
            );
        }
    }
}
