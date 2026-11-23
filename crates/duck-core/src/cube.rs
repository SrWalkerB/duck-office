//! Definições do Modelo de Dados (Power Pivot) como estão no arquivo: tabelas
//! e relações, medidas e o que cada tabela dinâmica usa dele. O cálculo fica
//! no `duck-dax`.

/// Tipo de uma coluna do modelo.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ModelKind {
    #[default]
    Any,
    Text,
    Number,
    Date,
    Bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ModelColumn {
    pub name: String,
    pub kind: ModelKind,
    /// Fórmula DAX de coluna calculada.
    pub expr: Option<String>,
    /// Coluna que dá a ordem dos membros ("classificar por coluna").
    pub sort_by: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ModelTable {
    pub name: String,
    pub columns: Vec<ModelColumn>,
    /// Tabela do Excel (ou intervalo) de onde vêm os dados.
    pub source: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ModelDef {
    pub tables: Vec<ModelTable>,
    /// (tabela, coluna) muitos → (tabela, coluna) um.
    pub rels: Vec<(String, String, String, String)>,
    /// (tabela, nome, fórmula).
    pub measures: Vec<(String, String, String)>,
}

/// Uma hierarquia do cache (`cacheHierarchy`): coluna ou medida.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Hierarchy {
    /// `[Tabela].[Coluna]` ou `[Measures].[Nome]`.
    pub unique: String,
    pub caption: String,
    pub measure: bool,
}

/// Campo do cache de uma dinâmica do modelo.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CubeField {
    /// `[Tabela].[Coluna].[Coluna]` ou `[Measures].[Nome]`.
    pub name: String,
    pub hierarchy: Option<u32>,
    pub caption: String,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct CubeCache {
    pub hierarchies: Vec<Hierarchy>,
    pub fields: Vec<CubeField>,
}

impl CubeCache {
    /// Hierarquia de um campo da tabela (as medidas não têm).
    pub fn hierarchy_of(&self, field: usize) -> Option<&Hierarchy> {
        self.hierarchies.get(self.fields.get(field)?.hierarchy? as usize)
    }

    /// Com os campos que entraram depois na tabela.
    pub fn with_new(&self, p: &CubePivot) -> CubeCache {
        let mut c = self.clone();
        for h in &p.new_fields {
            let Some(hier) = self.hierarchies.get(*h as usize) else { continue };
            let name = match split_unique(&hier.unique) {
                Some((t, col)) => format!("{}.{}.{}", bracket(&t), bracket(&col), bracket(&col)),
                None => hier.unique.clone(),
            };
            c.fields.push(CubeField { name, hierarchy: Some(*h), caption: hier.caption.clone() });
        }
        c
    }

    /// Campo de uma hierarquia, se ela está na tabela.
    pub fn field_of(&self, hierarchy: usize) -> Option<usize> {
        self.fields.iter().position(|f| f.hierarchy == Some(hierarchy as u32))
    }
}

/// O que é próprio de uma tabela dinâmica do modelo.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CubePivot {
    /// Filtros de página: (campo, hierarquia, membro escolhido).
    pub pages: Vec<(u32, i32, Option<String>)>,
    /// Membros escolhidos por hierarquia (`pivotHierarchy/members`).
    pub members: Vec<(u32, Vec<String>)>,
    /// Depois de recalcular: os membros dos campos dos eixos (trocam os do
    /// cache, que é só desta tabela).
    pub items: Vec<(u32, Vec<crate::pivot::PItem>)>,
    /// Hierarquias que entraram na tabela (viram campos no fim da lista).
    pub new_fields: Vec<u32>,
}

impl CubePivot {
    /// Escolhe os membros de uma hierarquia (`None` = todos).
    pub fn choose(&mut self, hierarchy: u32, unique: &str, members: Option<Vec<String>>) {
        self.members.retain(|m| m.0 != hierarchy);
        for p in self.pages.iter_mut().filter(|p| p.1 == hierarchy as i32) {
            p.2 = Some(match &members {
                Some(m) => m.first().cloned().unwrap_or_default(),
                None => format!("{unique}.[All]"),
            });
        }
        // Um membro só num filtro de página já está no próprio filtro.
        let page = self.pages.iter().any(|p| p.1 == hierarchy as i32);
        if let Some(m) = members
            && !(page && m.len() == 1)
        {
            let i = self.members.partition_point(|x| x.0 < hierarchy);
            self.members.insert(i, (hierarchy, m));
        }
    }
}

/// Parte `[Tabela].[Coluna]` de um nome único: (tabela, coluna).
pub fn split_unique(unique: &str) -> Option<(String, String)> {
    let mut parts = Vec::new();
    let mut rest = unique;
    while let Some(s) = rest.strip_prefix('[') {
        let mut name = String::new();
        let mut it = s.char_indices().peekable();
        let mut end = None;
        while let Some((i, c)) = it.next() {
            if c == ']' {
                if it.peek().is_some_and(|x| x.1 == ']') {
                    it.next();
                    name.push(']');
                    continue;
                }
                end = Some(i + 1);
                break;
            }
            name.push(c);
        }
        parts.push(name);
        rest = s[end?..].trim_start_matches('.');
        if rest.starts_with('&') {
            break;
        }
    }
    let mut it = parts.into_iter();
    Some((it.next()?, it.next()?))
}

/// Nome entre colchetes.
pub fn bracket(s: &str) -> String {
    format!("[{}]", s.replace(']', "]]"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn choose_members() {
        let mut p = CubePivot { pages: vec![(0, 7, Some("[T].[c].&[a]".into()))], ..Default::default() };
        p.choose(7, "[T].[c]", Some(vec!["[T].[c].&[b]".into()]));
        assert_eq!((p.pages[0].2.as_deref(), p.members.len()), (Some("[T].[c].&[b]"), 0));
        p.choose(7, "[T].[c]", Some(vec!["x".into(), "y".into()]));
        assert_eq!(p.members, [(7, vec!["x".to_string(), "y".to_string()])]);
        p.choose(7, "[T].[c]", None);
        assert_eq!((p.pages[0].2.as_deref(), p.members.len()), (Some("[T].[c].[All]"), 0));
        // Fora dos filtros de página, um membro só também vai na lista.
        p.choose(3, "[T].[d]", Some(vec!["z".into()]));
        assert_eq!(p.members.len(), 1);
    }

    #[test]
    fn unique_names() {
        let s = |u: &str| split_unique(u).unwrap();
        assert_eq!(s("[Vendas].[Loja].[Loja]"), ("Vendas".into(), "Loja".into()));
        assert_eq!(s("[V].[a]]b].&[x]"), ("V".into(), "a]b".into()));
        assert_eq!(s("[Measures].[Soma de x]"), ("Measures".into(), "Soma de x".into()));
        assert_eq!(split_unique("nada"), None);
        assert_eq!(bracket("a]b"), "[a]]b]");
    }
}
