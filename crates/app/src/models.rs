//! Actualizar listas sin rearmarlas: si la lista de la interfaz ya existe, solo
//! se cambian las filas distintas. Así las filas no se destruyen y recrean en
//! cada recarga: es más rápido y un clic que está ocurriendo no se pierde.

use slint::{Model, ModelRc, VecModel};

/// Devuelve el modelo a usar: el mismo (actualizado en el lugar) o uno nuevo.
pub fn sync<T: Clone + PartialEq + 'static>(current: ModelRc<T>, rows: Vec<T>) -> ModelRc<T> {
    let Some(model) = current.as_any().downcast_ref::<VecModel<T>>() else {
        return ModelRc::new(VecModel::from(rows));
    };
    let old_len = model.row_count();
    for (index, row) in rows.iter().enumerate().take(old_len) {
        if model.row_data(index).as_ref() != Some(row) {
            model.set_row_data(index, row.clone());
        }
    }
    if rows.len() > old_len {
        for row in rows.into_iter().skip(old_len) {
            model.push(row);
        }
    } else {
        for index in (rows.len()..old_len).rev() {
            model.remove(index);
        }
    }
    current
}

#[cfg(test)]
mod tests {
    use slint::{Model, ModelRc, VecModel};

    use super::sync;

    #[test]
    fn updates_in_place() {
        let first = sync(ModelRc::default(), vec![1, 2, 3]);
        let second = sync(first.clone(), vec![1, 5]);
        assert_eq!(second, first, "se reutiliza el mismo modelo");
        assert_eq!(second.iter().collect::<Vec<_>>(), vec![1, 5]);
        let third = sync(second.clone(), vec![1, 5, 7, 9]);
        assert_eq!(third.iter().collect::<Vec<_>>(), vec![1, 5, 7, 9]);
        let _ = VecModel::<i32>::default();
    }
}
