//! Tests of the language and the library. Results are compared as M source (`render`).

use std::rc::Rc;

use crate::{Host, Interp, NoHost, Now, Slot, Table, Value, render};

fn run_with(host: &dyn Host, src: &str) -> String {
    let mut it = Interp::new(host);
    match it.eval_text(src) {
        Ok(v) => render(&v),
        Err(e) => format!("ERROR {e}"),
    }
}

fn run(src: &str) -> String {
    run_with(&NoHost, src)
}

/// Checks `source => expected` pairs.
fn check(cases: &[(&str, &str)]) {
    for (src, want) in cases {
        assert_eq!(run(src), *want, "source: {src}");
    }
}

fn fails(src: &str, with: &str) {
    let got = run(src);
    assert!(got.starts_with("ERROR") && got.contains(with), "source: {src}\n   got: {got}\n  want an error containing: {with}");
}

#[test]
fn literals_and_operators() {
    check(&[
        ("1 + 2 * 3", "7"),
        ("(1 + 2) * 3", "9"),
        ("10 / 4", "2.5"),
        ("-2 + 5", "3"),
        ("- -2", "2"),
        ("0x1F", "31"),
        ("1.5e3", "1500"),
        (".5 + .5", "1"),
        ("1 / 0", "#infinity"),
        ("-1 / 0", "-#infinity"),
        ("0 / 0", "#nan"),
        ("#nan = #nan", "false"),
        ("\"a\" & \"b\" & \"c\"", "\"abc\""),
        ("\"say \"\"hi\"\"\"", "\"say \"\"hi\"\"\""),
        ("\"line#(lf)tab#(tab)\"", "\"line#(lf)tab#(tab)\""),
        ("\"#(0041)#(cr,lf)\"", "\"A#(cr)#(lf)\""),
        ("1 = 1", "true"),
        ("1 <> 1", "false"),
        ("\"a\" < \"b\"", "true"),
        ("2 >= 3", "false"),
        ("true and false", "false"),
        ("true or false", "true"),
        ("not true", "false"),
        ("null = null", "true"),
        ("null = 1", "false"),
        ("1 + null", "null"),
        ("null < 1", "null"),
        ("\"a\" & null", "null"),
        ("null ?? 5", "5"),
        ("3 ?? 5", "3"),
        ("null ?? null ?? 7", "7"),
        // Three-valued logic.
        ("null and false", "false"),
        ("null and true", "null"),
        ("null or true", "true"),
        ("null or false", "null"),
        ("false and (1 / 0 > \"x\")", "false"),
        ("true or error \"never\"", "true"),
        ("not null", "null"),
        ("1 /* block */ + // line\n 2", "3"),
    ]);
    fails("1 + \"a\"", "operator + can't be applied");
    fails("1 < \"a\"", "can't be compared");
    fails("if null then 1 else 2", "Logical");
    fails("1 and true", "Logical");
}

#[test]
fn lists_records_and_access() {
    check(&[
        ("{1, 2, 3}", "{1, 2, 3}"),
        ("{1..5}", "{1, 2, 3, 4, 5}"),
        ("{5..1}", "{}"),
        ("{1, 3..5, 9}", "{1, 3, 4, 5, 9}"),
        ("{\"a\"..\"d\"}", "{\"a\", \"b\", \"c\", \"d\"}"),
        ("{1, 2, 3}{1}", "2"),
        ("{1, 2, 3}{5}?", "null"),
        ("{1, 2} & {3}", "{1, 2, 3}"),
        ("{1, 2} = {1, 2}", "true"),
        ("{1, {2, 3}} = {1, {2, 4}}", "false"),
        ("[a = 1, b = 2]", "[a = 1, b = 2]"),
        ("[a = 1, b = 2][b]", "2"),
        ("[a = 1][c]?", "null"),
        ("[a = 1, b = a + 1][b]", "2"),
        ("[b = a + 1, a = 1][b]", "2"),
        ("[a = 1, b = 2, c = 3][[a], [c]]", "[a = 1, c = 3]"),
        ("[a = 1] & [b = 2] & [a = 9]", "[a = 9, b = 2]"),
        ("[a = 1, b = 2] = [b = 2, a = 1]", "true"),
        ("[#\"Unit Price\" = 2, Qty = 3][Unit Price]", "2"),
        ("[Unit Price = 2, Qty = 3][#\"Unit Price\"] * 2", "4"),
        ("[2024 Sales = 5][2024 Sales]", "5"),
        ("[]", "[]"),
        ("{}", "{}"),
        ("List.Count({1, error \"x\", 3})", "3"),
        ("null[a]?", "null"),
    ]);
    fails("{1, 2}{5}", "no item 5");
    fails("{1, error \"boom\"}{1}", "boom");
    fails("[a = 1][b]", "no field named 'b'");
    fails("[a = b, b = a][a]", "cyclic");
    fails("{1, 2}{-1}", "isn't negative");
}

#[test]
fn let_functions_and_scope() {
    check(&[
        ("let a = 1, b = a + 1 in b * 2", "4"),
        ("let b = a + 1, a = 1 in b", "2"),
        ("let f = (x) => x * 2 in f(4)", "8"),
        ("let f = (x, optional y) => if y = null then x else x + y in {f(1), f(1, 2)}", "{1, 3}"),
        ("let f = (x as number) as number => x + 1 in f(1)", "2"),
        ("(each _ + 1)(2)", "3"),
        ("(each [a] + [b])([a = 1, b = 2])", "3"),
        ("List.Transform({1, 2}, each _ * 10)", "{10, 20}"),
        // Recursion through @.
        ("let fact = (n) => if n <= 1 then 1 else n * @fact(n - 1) in fact(10)", "3628800"),
        ("let fib = (n) => if n < 2 then n else @fib(n - 1) + @fib(n - 2) in fib(15)", "610"),
        ("[f = (n) => if n = 0 then 0 else n + @f(n - 1)][f](4)", "10"),
        // A step may shadow a name with a value built from the outer one.
        ("let x = 1, y = let x = x + 1 in x in y", "2"),
        ("let f = (t) => let t = t * 2 in t in f(21)", "42"),
        ("let x = 1 in [x = x + 1][x]", "2"),
        // Closures capture their scope.
        ("let add = (a) => (b) => a + b, add5 = add(5) in add5(3)", "8"),
        ("let a = 1, f = () => a, a2 = 2 in f()", "1"),
        // Unused steps aren't evaluated.
        ("let bad = error \"unused\", good = 1 in good", "1"),
        ("let #\"Changed Type\" = 5 in #\"Changed Type\" + 1", "6"),
        ("if 1 > 2 then \"a\" else \"b\"", "\"b\""),
        ("1 + if true then 1 else 2", "2"),
        ("let Table.Mine = 3 in Table.Mine", "3"),
    ]);
    fails("let f = (x as number) => x in f(\"a\")", "expects Number");
    fails("let f = (x) => x in f(1, 2)", "takes 1 argument");
    fails("let f = (x) => x in f()", "takes 1 argument");
    fails("nope + 1", "'nope' isn't recognized");
    fails("let a = a + 1 in a", "'a' isn't recognized");
    fails("let a = @a + 1 in a", "cyclic");
    fails("5(1)", "can't be called");
}

#[test]
fn errors_and_try() {
    check(&[
        ("try 1", "[HasError = false, Value = 1]"),
        ("(try error \"boom\")[HasError]", "true"),
        ("(try error \"boom\")[Error][Message]", "\"boom\""),
        ("(try error \"boom\")[Error][Reason]", "\"Expression.Error\""),
        ("try error \"boom\" otherwise 7", "7"),
        ("try 1 otherwise 7", "1"),
        ("try error \"boom\" catch (e) => e[Message] & \"!\"", "\"boom!\""),
        ("try error \"boom\" catch () => 0", "0"),
        ("try error [Reason = \"My.Error\", Message = \"m\", Detail = 5] catch (e) => {e[Reason], e[Detail]}", "{\"My.Error\", 5}"),
        ("try error Error.Record(\"R\", \"M\", {1}) catch (e) => e[Detail]", "{1}"),
        ("try Number.FromText(\"abc\") catch (e) => e[Reason]", "\"DataFormat.Error\""),
        ("try {1, 2}{9} otherwise \"none\"", "\"none\""),
        ("try 1 + (try error \"x\" otherwise 2)", "[HasError = false, Value = 3]"),
        ("try ... otherwise \"todo\"", "\"todo\""),
    ]);
    fails("error \"plain\"", "plain");
}

#[test]
fn types() {
    check(&[
        ("1 is number", "true"),
        ("\"a\" is number", "false"),
        ("null is nullable number", "true"),
        ("null is number", "false"),
        ("{1} is list", "true"),
        ("[a = 1] is record", "true"),
        ("#table({\"a\"}, {}) is table", "true"),
        ("(each _) is function", "true"),
        ("1 as number", "1"),
        ("null as nullable text", "null"),
        ("type number", "type number"),
        ("Value.Type(1) = type number", "true"),
        ("Value.Is(1, type number)", "true"),
        ("Value.Is(\"a\", Int64.Type)", "false"),
        ("Type.Is(type number, type any)", "true"),
        ("Type.Is(type any, type number)", "false"),
        ("Type.IsNullable(type nullable number)", "true"),
        ("Type.NonNullable(type nullable number) = type number", "true"),
        ("Type.ListItem(type {text}) = type text", "true"),
        ("Record.FieldNames(Type.RecordFields(type [a = number, optional b = text]))", "{\"a\", \"b\"}"),
        ("Type.TableColumn(type table [a = number, b = text], \"b\") = type text", "true"),
        ("Table.ColumnNames(#table(type table [a = number, b = text], {}))", "{\"a\", \"b\"}"),
        ("type function (x as number) as number", "type function"),
    ]);
    fails("1 as text", "can't be converted to type Text");
}

#[test]
fn text_functions() {
    check(&[
        ("Text.Length(\"héllo\")", "5"),
        ("Text.Upper(\"abc\")", "\"ABC\""),
        ("Text.Lower(\"ABC\")", "\"abc\""),
        ("Text.Proper(\"the quick-brown fox\")", "\"The Quick-Brown Fox\""),
        ("Text.Trim(\"  a b  \")", "\"a b\""),
        ("Text.Trim(\"xxaxx\", \"x\")", "\"a\""),
        ("Text.TrimStart(\"  a \")", "\"a \""),
        ("Text.TrimEnd(\"  a \")", "\"  a\""),
        ("Text.Start(\"Hello\", 2)", "\"He\""),
        ("Text.End(\"Hello\", 3)", "\"llo\""),
        ("Text.Middle(\"Hello\", 1, 3)", "\"ell\""),
        ("Text.Middle(\"Hello\", 9)", "\"\""),
        ("Text.Range(\"Hello\", 1, 2)", "\"el\""),
        ("Text.At(\"Hello\", 1)", "\"e\""),
        ("Text.Contains(\"Hello\", \"ell\")", "true"),
        ("Text.Contains(\"Hello\", \"ELL\")", "false"),
        ("Text.Contains(\"Hello\", \"ELL\", Comparer.OrdinalIgnoreCase)", "true"),
        ("Text.StartsWith(\"Hello\", \"He\")", "true"),
        ("Text.EndsWith(\"Hello\", \"lo\")", "true"),
        ("Text.PositionOf(\"Hello\", \"l\")", "2"),
        ("Text.PositionOf(\"Hello\", \"l\", Occurrence.Last)", "3"),
        ("Text.PositionOf(\"Hello\", \"l\", Occurrence.All)", "{2, 3}"),
        ("Text.PositionOf(\"Hello\", \"z\")", "-1"),
        ("Text.PositionOfAny(\"Hello\", {\"l\", \"o\"})", "2"),
        ("Text.Replace(\"a-b-c\", \"-\", \"+\")", "\"a+b+c\""),
        ("Text.ReplaceRange(\"Hello\", 1, 3, \"EY\")", "\"HEYo\""),
        ("Text.RemoveRange(\"Hello\", 1, 3)", "\"Ho\""),
        ("Text.Insert(\"Hlo\", 1, \"el\")", "\"Hello\""),
        ("Text.Remove(\"a1b2\", {\"1\", \"2\"})", "\"ab\""),
        ("Text.Select(\"a1b2\", {\"0\"..\"9\"})", "\"12\""),
        ("Text.Split(\"a,b,,c\", \",\")", "{\"a\", \"b\", \"\", \"c\"}"),
        ("Text.SplitAny(\"a,b;c\", \",;\")", "{\"a\", \"b\", \"c\"}"),
        ("Text.Combine({\"a\", null, \"b\"}, \"-\")", "\"a-b\""),
        ("Text.PadStart(\"7\", 3, \"0\")", "\"007\""),
        ("Text.PadEnd(\"7\", 3)", "\"7  \""),
        ("Text.Repeat(\"ab\", 3)", "\"ababab\""),
        ("Text.Reverse(\"abc\")", "\"cba\""),
        ("Text.ToList(\"abc\")", "{\"a\", \"b\", \"c\"}"),
        ("Text.BeforeDelimiter(\"a-b-c\", \"-\")", "\"a\""),
        ("Text.BeforeDelimiter(\"a-b-c\", \"-\", 1)", "\"a-b\""),
        ("Text.BeforeDelimiter(\"a-b-c\", \"-\", {0, RelativePosition.FromEnd})", "\"a-b\""),
        ("Text.AfterDelimiter(\"a-b-c\", \"-\")", "\"b-c\""),
        ("Text.AfterDelimiter(\"abc\", \"-\")", "\"\""),
        ("Text.BetweenDelimiters(\"x(a)y(b)\", \"(\", \")\")", "\"a\""),
        ("Text.BetweenDelimiters(\"x(a)y(b)\", \"(\", \")\", 1)", "\"b\""),
        ("Text.Format(\"#{0} of #{1}\", {1, 2})", "\"1 of 2\""),
        ("Text.Format(\"#[n] items\", [n = 3])", "\"3 items\""),
        ("Text.From(1.5)", "\"1.5\""),
        ("Text.From(true)", "\"true\""),
        ("Text.From(null)", "null"),
        ("Text.From(1.5, \"pt-BR\")", "\"1,5\""),
        ("Text.Upper(null)", "null"),
        ("Text.Clean(\"a#(tab)b\")", "\"ab\""),
        ("Character.FromNumber(65)", "\"A\""),
        ("Character.ToNumber(\"A\")", "65"),
    ]);
    fails("Text.Upper(1)", "text is expected");
    fails("Text.Range(\"abc\", 2, 5)", "outside the text");
}

#[test]
fn number_functions() {
    check(&[
        ("Number.Round(2.5)", "2"),
        ("Number.Round(3.5)", "4"),
        ("Number.Round(2.5, 0, RoundingMode.AwayFromZero)", "3"),
        ("Number.Round(1.005, 2, RoundingMode.AwayFromZero)", "1.01"),
        ("Number.Round(1234.5678, 2)", "1234.57"),
        ("Number.Round(1234.5678, -2)", "1200"),
        ("Number.RoundUp(1.1)", "2"),
        ("Number.RoundDown(1.9)", "1"),
        ("Number.RoundDown(-1.1)", "-2"),
        ("Number.RoundTowardZero(-1.9)", "-1"),
        ("Number.RoundAwayFromZero(-1.1)", "-2"),
        ("Number.Abs(-3)", "3"),
        ("Number.Sign(-3)", "-1"),
        ("Number.Sqrt(16)", "4"),
        ("Number.Power(2, 10)", "1024"),
        ("Number.Mod(7, 3)", "1"),
        ("Number.IntegerDivide(7, 2)", "3"),
        ("Number.IsEven(4)", "true"),
        ("Number.IsOdd(4)", "false"),
        ("Number.Factorial(5)", "120"),
        ("Number.Combinations(5, 2)", "10"),
        ("Number.Permutations(5, 2)", "20"),
        ("Number.BitwiseAnd(12, 10)", "8"),
        ("Number.BitwiseXor(12, 10)", "6"),
        ("Number.From(\"1,234.5\")", "1234.5"),
        ("Number.From(\"1.234,5\", \"pt-BR\")", "1234.5"),
        ("Number.From(\"50%\")", "0.5"),
        ("Number.From(\"(12)\")", "-12"),
        ("Number.From(\"R$ 10,50\", \"pt-BR\")", "10.5"),
        ("Number.From(true)", "1"),
        ("Number.From(#date(2024, 1, 1))", "45292"),
        ("Number.From(null)", "null"),
        ("Number.FromText(\"1e3\")", "1000"),
        ("Int64.From(2.5)", "2"),
        ("Int64.From(3.5)", "4"),
        ("Int64.From(\"42\")", "42"),
        ("Currency.From(1.23456)", "1.2346"),
        ("Number.ToText(1234.5, \"N2\")", "\"1,234.50\""),
        ("Number.ToText(1234.5, \"N2\", \"pt-BR\")", "\"1.234,50\""),
        ("Number.ToText(0.256, \"P1\")", "\"25.6%\""),
        ("Number.ToText(5, \"D3\")", "\"005\""),
        ("Number.ToText(255, \"X\")", "\"FF\""),
        ("Number.ToText(1234.5, \"F1\")", "\"1234.5\""),
        ("Number.ToText(1234.567, \"0.00\")", "\"1234.57\""),
        ("Number.ToText(1234.567, \"#,##0.0\")", "\"1,234.6\""),
        ("Number.ToText(7, \"000\")", "\"007\""),
        ("Number.ToText(0.5, \"0%\")", "\"50%\""),
        ("Number.ToText(12345.678, \"E2\")", "\"1.23E+004\""),
        ("Number.ToText(1e21)", "\"1E+21\""),
        ("Number.ToText(0.000001)", "\"1E-06\""),
        ("Number.PI > 3.14 and Number.PI < 3.15", "true"),
        ("Number.IsNaN(0 / 0)", "true"),
        ("Logical.From(\"TRUE\")", "true"),
        ("Logical.From(0)", "false"),
        ("Logical.ToText(true)", "\"true\""),
    ]);
    fails("Number.From(\"abc\")", "can't be read as a number");
    fails("Int8.From(300)", "out of range");
}

#[test]
fn list_functions() {
    check(&[
        ("List.Sum({1, 2, null, 3})", "6"),
        ("List.Sum({})", "null"),
        ("List.Average({1, 2, 3, 6})", "3"),
        ("List.Min({3, 1, 2})", "1"),
        ("List.Max({3, 1, 2})", "3"),
        ("List.Max({})", "null"),
        ("List.Max({}, 0)", "0"),
        ("List.Max({\"b\", \"a\"})", "\"b\""),
        ("List.Product({2, 3, 4})", "24"),
        ("List.Median({5, 1, 3})", "3"),
        ("List.Median({1, 2, 3, 4})", "2.5"),
        ("List.Mode({1, 2, 2, 3})", "2"),
        ("List.StandardDeviation({2, 4, 4, 4, 5, 5, 7, 9})", "2.138089935299395"),
        ("List.Count({1, null})", "2"),
        ("List.NonNullCount({1, null})", "1"),
        ("List.IsEmpty({})", "true"),
        ("List.First({1, 2})", "1"),
        ("List.First({}, 9)", "9"),
        ("List.Last({1, 2})", "2"),
        ("List.FirstN({1, 2, 3}, 2)", "{1, 2}"),
        ("List.FirstN({1, 2, 3, 1}, each _ < 3)", "{1, 2}"),
        ("List.LastN({1, 2, 3}, 2)", "{2, 3}"),
        ("List.Skip({1, 2, 3})", "{2, 3}"),
        ("List.Skip({1, 2, 3}, 2)", "{3}"),
        ("List.RemoveLastN({1, 2, 3}, 1)", "{1, 2}"),
        ("List.Range({1, 2, 3, 4}, 1, 2)", "{2, 3}"),
        ("List.Reverse({1, 2, 3})", "{3, 2, 1}"),
        ("List.RemoveNulls({1, null, 2})", "{1, 2}"),
        ("List.Distinct({1, 2, 1, 3, 2})", "{1, 2, 3}"),
        ("List.Distinct({\"a\", \"A\"}, Comparer.OrdinalIgnoreCase)", "{\"a\"}"),
        ("List.IsDistinct({1, 2, 1})", "false"),
        ("List.Contains({1, 2}, 2)", "true"),
        ("List.ContainsAny({1, 2}, {5, 2})", "true"),
        ("List.ContainsAll({1, 2}, {5, 2})", "false"),
        ("List.PositionOf({\"a\", \"b\", \"a\"}, \"a\", Occurrence.All)", "{0, 2}"),
        ("List.PositionOf({1, 2}, 9)", "-1"),
        ("List.Positions({\"a\", \"b\"})", "{0, 1}"),
        ("List.Select({1, 2, 3, 4}, each Number.IsEven(_))", "{2, 4}"),
        ("List.Accumulate({1, 2, 3}, 0, (s, c) => s + c)", "6"),
        ("List.Generate(() => 1, each _ < 50, each _ * 2)", "{1, 2, 4, 8, 16, 32}"),
        ("List.Generate(() => 1, each _ <= 3, each _ + 1, each _ * 10)", "{10, 20, 30}"),
        ("List.Numbers(1, 4, 2)", "{1, 3, 5, 7}"),
        ("List.Repeat({1, 2}, 2)", "{1, 2, 1, 2}"),
        ("List.Combine({{1}, {2, 3}})", "{1, 2, 3}"),
        ("List.Zip({{1, 2}, {\"a\", \"b\"}})", "{{1, \"a\"}, {2, \"b\"}}"),
        ("List.Sort({3, 1, 2})", "{1, 2, 3}"),
        ("List.Sort({3, 1, 2}, Order.Descending)", "{3, 2, 1}"),
        ("List.Sort({\"bb\", \"a\", \"ccc\"}, each Text.Length(_))", "{\"a\", \"bb\", \"ccc\"}"),
        ("List.Sort({\"bb\", \"a\", \"ccc\"}, {each Text.Length(_), Order.Descending})", "{\"ccc\", \"bb\", \"a\"}"),
        ("List.Sort({1, 3, 2}, (x, y) => Value.Compare(y, x))", "{3, 2, 1}"),
        ("List.Sort({2, null, 1})", "{null, 1, 2}"),
        // An inconsistent comparer must not bring the sort down.
        ("List.Count(List.Sort({5, 3, 9, 1, 7, 2, 8}, (x, y) => 1))", "7"),
        ("List.Union({{1, 2}, {2, 3}})", "{1, 2, 3}"),
        ("List.Intersect({{1, 2, 3}, {2, 3, 4}})", "{2, 3}"),
        ("List.Difference({1, 2, 2, 3}, {2})", "{1, 2, 3}"),
        ("List.RemoveItems({1, 2, 2, 3}, {2})", "{1, 3}"),
        ("List.ReplaceValue({1, 2, 1}, 1, 9, Replacer.ReplaceValue)", "{9, 2, 9}"),
        ("List.ReplaceMatchingItems({1, 2, 3}, {{1, 10}, {3, 30}})", "{10, 2, 30}"),
        ("List.ReplaceRange({1, 2, 3}, 1, 1, {8, 9})", "{1, 8, 9, 3}"),
        ("List.RemoveRange({1, 2, 3}, 0, 2)", "{3}"),
        ("List.InsertRange({1, 4}, 1, {2, 3})", "{1, 2, 3, 4}"),
        ("List.AnyTrue({false, true})", "true"),
        ("List.AllTrue({false, true})", "false"),
        ("List.MatchesAny({1, 5}, each _ > 4)", "true"),
        ("List.MatchesAll({1, 5}, each _ > 4)", "false"),
        ("List.Single({7})", "7"),
        ("List.SingleOrDefault({}, 0)", "0"),
        ("List.FindText({\"apple\", \"Banana\", 3}, \"an\")", "{\"Banana\"}"),
        ("List.Split({1, 2, 3, 4, 5}, 2)", "{{1, 2}, {3, 4}, {5}}"),
        ("List.Alternate({1, 2, 3, 4, 5, 6}, 1, 1)", "{2, 4, 6}"),
        ("List.TransformMany({1, 2}, (x) => {x, x * 10}, (x, y) => x + y)", "{2, 11, 4, 22}"),
        ("List.MaxN({1, 5, 3, 4}, 2)", "{5, 4}"),
        ("List.MinN({1, 5, 3, 4}, 2)", "{1, 3}"),
        ("List.Dates(#date(2024, 1, 30), 3, #duration(1, 0, 0, 0))", "{#date(2024, 1, 30), #date(2024, 1, 31), #date(2024, 2, 1)}"),
    ]);
    fails("List.Single({1, 2})", "more than one item");
    fails("List.Sum({1, \"a\"})", "operator +");
}

#[test]
fn record_functions() {
    check(&[
        ("Record.Field([a = 1], \"a\")", "1"),
        ("Record.FieldOrDefault([a = 1], \"b\", 0)", "0"),
        ("Record.FieldNames([a = 1, b = 2])", "{\"a\", \"b\"}"),
        ("Record.FieldValues([a = 1, b = 2])", "{1, 2}"),
        ("Record.FieldCount([a = 1, b = 2])", "2"),
        ("Record.HasFields([a = 1], {\"a\", \"b\"})", "false"),
        ("Record.AddField([a = 1], \"b\", 2)", "[a = 1, b = 2]"),
        ("Record.RemoveFields([a = 1, b = 2], \"a\")", "[b = 2]"),
        ("Record.RenameFields([a = 1, b = 2], {{\"a\", \"x\"}})", "[x = 1, b = 2]"),
        ("Record.SelectFields([a = 1, b = 2, c = 3], {\"c\", \"a\"})", "[c = 3, a = 1]"),
        ("Record.ReorderFields([a = 1, b = 2, c = 3], {\"c\", \"a\"})", "[c = 3, b = 2, a = 1]"),
        ("Record.TransformFields([a = 1, b = 2], {\"a\", each _ + 10})", "[a = 11, b = 2]"),
        ("Record.Combine({[a = 1], [b = 2], [a = 3]})", "[a = 3, b = 2]"),
        ("Record.FromList({1, 2}, {\"a\", \"b\"})", "[a = 1, b = 2]"),
        ("Record.ToTable([a = 1, b = 2])", "#table({\"Name\", \"Value\"}, {{\"a\", 1}, {\"b\", 2}})"),
        ("Record.FromTable(#table({\"Name\", \"Value\"}, {{\"a\", 1}}))", "[a = 1]"),
    ]);
    fails("Record.Field([a = 1], \"z\")", "no field named 'z'");
    fails("Record.RemoveFields([a = 1], \"z\")", "no field named 'z'");
}

const SALES: &str = "#table({\"Region\", \"Product\", \"Qty\", \"Price\"}, {{\"North\", \"Pen\", 10, 1.5}, {\"South\", \"Pen\", 4, 1.5}, {\"North\", \"Ink\", 3, 8}, {\"East\", \"Pad\", null, 3}})";

/// Evaluates `expr` with `Sales` bound to a small table.
fn sales(expr: &str) -> String {
    run(&format!("let Sales = {SALES} in {expr}"))
}

#[test]
fn table_basics() {
    assert_eq!(run("#table({\"a\", \"b\"}, {{1, 2}, {3, 4}})"), "#table({\"a\", \"b\"}, {{1, 2}, {3, 4}})");
    assert_eq!(run("#table(2, {{1, 2}})"), "#table({\"Column1\", \"Column2\"}, {{1, 2}})");
    assert_eq!(run("Table.FromRecords({[a = 1, b = 2], [a = 3, b = 4]})"), "#table({\"a\", \"b\"}, {{1, 2}, {3, 4}})");
    assert_eq!(run("Table.FromRows({{1, 2}}, {\"x\", \"y\"})"), "#table({\"x\", \"y\"}, {{1, 2}})");
    assert_eq!(run("Table.FromColumns({{1, 2}, {\"a\"}}, {\"n\", \"t\"})"), "#table({\"n\", \"t\"}, {{1, \"a\"}, {2, null}})");
    assert_eq!(run("Table.FromList({\"a,1\", \"b,2\"}, null, {\"L\", \"N\"})"), "#table({\"L\", \"N\"}, {{\"a\", \"1\"}, {\"b\", \"2\"}})");
    assert_eq!(run("Table.FromList({1, 2}, Splitter.SplitByNothing())"), "#table({\"Column1\"}, {{1}, {2}})");
    assert_eq!(sales("Table.RowCount(Sales)"), "4");
    assert_eq!(sales("Table.ColumnCount(Sales)"), "4");
    assert_eq!(sales("Table.ColumnNames(Sales)"), "{\"Region\", \"Product\", \"Qty\", \"Price\"}");
    assert_eq!(sales("Sales[Qty]"), "{10, 4, 3, null}");
    assert_eq!(sales("Sales{0}[Product]"), "\"Pen\"");
    assert_eq!(sales("Sales{[Product = \"Ink\"]}[Qty]"), "3");
    assert_eq!(sales("Sales{[Product = \"Nope\"]}?"), "null");
    assert_eq!(sales("Table.Column(Sales, \"Region\")"), "{\"North\", \"South\", \"North\", \"East\"}");
    assert_eq!(sales("Table.First(Sales)[Region]"), "\"North\"");
    assert_eq!(sales("Table.Last(Sales)[Region]"), "\"East\"");
    assert_eq!(sales("Table.IsEmpty(Sales)"), "false");
    assert_eq!(sales("Table.ToRows(Table.FirstN(Sales, 1))"), "{{\"North\", \"Pen\", 10, 1.5}}");
    assert_eq!(sales("Table.ToRecords(Sales){1}"), "[Region = \"South\", Product = \"Pen\", Qty = 4, Price = 1.5]");
    assert_eq!(sales("Sales[[Region], [Qty]]{0}"), "[Region = \"North\", Qty = 10]");
    assert_eq!(sales("Table.HasColumns(Sales, {\"Qty\", \"Nope\"})"), "false");
    assert!(sales("Sales{[Product = \"Pen\"]}").contains("more than one row"));
    assert!(sales("Sales[Nope]").contains("no column named 'Nope'"));
    assert!(run("#table({\"a\", \"a\"}, {})").contains("two columns named 'a'"));
    assert!(run("#table({\"a\"}, {{1, 2}})").contains("1 column(s)"));
}

#[test]
fn table_rows_and_columns() {
    assert_eq!(sales("Table.SelectRows(Sales, each [Qty] > 3)[Product]"), "{\"Pen\", \"Pen\"}");
    assert_eq!(sales("Table.SelectRows(Sales, each [Region] = \"North\" and [Price] > 2)[Product]"), "{\"Ink\"}");
    assert_eq!(sales("Table.SelectRows(Sales, each [Qty] <> null)[Qty]"), "{10, 4, 3}");
    assert_eq!(sales("Table.SelectRows(Sales, each Text.StartsWith([Product], \"P\"))[Region]"), "{\"North\", \"South\", \"East\"}");
    assert_eq!(sales("Table.FirstN(Sales, 2)[Qty]"), "{10, 4}");
    assert_eq!(sales("Table.FirstN(Sales, each [Product] = \"Pen\")[Qty]"), "{10, 4}");
    assert_eq!(sales("Table.LastN(Sales, 1)[Region]"), "{\"East\"}");
    assert_eq!(sales("Table.Skip(Sales, 3)[Region]"), "{\"East\"}");
    assert_eq!(sales("Table.RemoveLastN(Sales, 3)[Region]"), "{\"North\"}");
    assert_eq!(sales("Table.Range(Sales, 1, 2)[Qty]"), "{4, 3}");
    assert_eq!(sales("Table.RemoveRows(Sales, 0, 3)[Region]"), "{\"East\"}");
    assert_eq!(sales("Table.ReverseRows(Sales)[Region]"), "{\"East\", \"North\", \"South\", \"North\"}");
    assert_eq!(sales("Table.ColumnNames(Table.SelectColumns(Sales, {\"Qty\", \"Region\"}))"), "{\"Qty\", \"Region\"}");
    assert_eq!(sales("Table.ColumnNames(Table.RemoveColumns(Sales, {\"Qty\", \"Price\"}))"), "{\"Region\", \"Product\"}");
    assert_eq!(
        sales("Table.ColumnNames(Table.RenameColumns(Sales, {{\"Qty\", \"Quantity\"}}))"),
        "{\"Region\", \"Product\", \"Quantity\", \"Price\"}"
    );
    assert_eq!(sales("Table.ColumnNames(Table.ReorderColumns(Sales, {\"Price\", \"Region\"}))"), "{\"Price\", \"Product\", \"Qty\", \"Region\"}");
    assert_eq!(sales("Table.AddColumn(Sales, \"Total\", each [Qty] * [Price])[Total]"), "{15, 6, 24, null}");
    assert_eq!(sales("Table.AddIndexColumn(Sales, \"i\", 1, 1)[i]"), "{1, 2, 3, 4}");
    assert_eq!(sales("Table.DuplicateColumn(Sales, \"Qty\", \"Q2\")[Q2]"), "{10, 4, 3, null}");
    assert_eq!(
        sales("Table.TransformColumns(Sales, {{\"Product\", Text.Upper}, {\"Price\", each _ * 2}})[[Product], [Price]]{2}"),
        "[Product = \"INK\", Price = 16]"
    );
    assert_eq!(sales("Table.ReplaceValue(Sales, null, 0, Replacer.ReplaceValue, {\"Qty\"})[Qty]"), "{10, 4, 3, 0}");
    assert_eq!(
        sales("Table.ReplaceValue(Sales, \"Pen\", \"Pencil\", Replacer.ReplaceText, {\"Product\"})[Product]"),
        "{\"Pencil\", \"Pencil\", \"Ink\", \"Pad\"}"
    );
    assert_eq!(
        sales("Table.ReplaceValue(Sales, each [Product], each [Region], Replacer.ReplaceValue, {\"Product\"})[Product]"),
        "{\"North\", \"South\", \"North\", \"East\"}"
    );
    assert_eq!(sales("Table.Sort(Sales, {{\"Qty\", Order.Descending}})[Qty]"), "{10, 4, 3, null}");
    assert_eq!(sales("Table.Sort(Sales, \"Qty\")[Qty]"), "{null, 3, 4, 10}");
    assert_eq!(sales("Table.Sort(Sales, {{\"Region\", Order.Ascending}, {\"Qty\", Order.Descending}})[Qty]"), "{null, 10, 3, 4}");
    assert_eq!(sales("Table.Distinct(Sales, \"Product\")[Product]"), "{\"Pen\", \"Ink\", \"Pad\"}");
    assert_eq!(sales("Table.RowCount(Table.Distinct(Table.SelectColumns(Sales, {\"Region\"})))"), "3");
    assert_eq!(sales("Table.Contains(Sales, [Product = \"Ink\", Region = \"North\"])"), "true");
    assert_eq!(
        sales("Table.FillDown(Table.ReplaceValue(Sales, \"South\", null, Replacer.ReplaceValue, {\"Region\"}), {\"Region\"})[Region]"),
        "{\"North\", \"North\", \"North\", \"East\"}"
    );
    assert_eq!(sales("Table.FillUp(Sales, {\"Qty\"})[Qty]"), "{10, 4, 3, null}");
    assert_eq!(sales("Table.Max(Sales, \"Qty\")[Product]"), "\"Pen\"");
    assert_eq!(sales("Table.Min(Sales, \"Price\")[Region]"), "\"North\"");
    assert_eq!(sales("Table.MatchesAnyRows(Sales, each [Qty] = 3)"), "true");
    assert_eq!(sales("Table.TransformRows(Table.FirstN(Sales, 2), each [Region] & \"/\" & [Product])"), "{\"North/Pen\", \"South/Pen\"}");
    assert_eq!(sales("Table.ToList(Table.FirstN(Sales[[Region], [Product]], 1))"), "{\"North,Pen\"}");
    assert!(sales("Table.AddColumn(Sales, \"Qty\", each 1)").contains("already has a column named 'Qty'"));
    assert!(sales("Table.RemoveColumns(Sales, \"Nope\")").contains("no column named 'Nope'"));
    assert_eq!(sales("Table.ColumnCount(Table.RemoveColumns(Sales, \"Nope\", MissingField.Ignore))"), "4");
}

#[test]
fn table_types_and_errors() {
    let t = "Table.TransformColumnTypes(#table({\"n\", \"d\", \"t\"}, {{\"12\", \"2024-01-31\", 5}, {\"x\", \"31/01/2024\", null}}), {{\"n\", Int64.Type}, {\"d\", type date}, {\"t\", type text}})";
    assert_eq!(run(&format!("{t}[t]")), "{\"5\", null}");
    assert_eq!(run(&format!("{t}{{0}}[n]")), "12");
    // A cell that can't be converted holds an error; the rest of the table is fine.
    assert_eq!(run(&format!("Table.RowCount({t})")), "2");
    assert!(run(&format!("{t}{{1}}[n]")).contains("can't be read as a number"));
    assert_eq!(run(&format!("{t}{{0}}[d]")), "#date(2024, 1, 31)");
    assert!(run(&format!("{t}{{1}}[d]")).contains("can't be read as a date"));
    assert_eq!(run(&format!("Table.RowCount(Table.RemoveRowsWithErrors({t}))")), "1");
    assert_eq!(run(&format!("Table.RowCount(Table.SelectRowsWithErrors({t}))")), "1");
    assert_eq!(run(&format!("Table.ReplaceErrorValues({t}, {{{{\"n\", 0}}, {{\"d\", null}}}})[n]")), "{12, 0}");
    assert_eq!(run(&format!("Table.Schema({t})[TypeName]")), "{\"Int64.Type\", \"Date.Type\", \"Text.Type\"}");
    // The culture decides how text is read.
    let br =
        "Table.TransformColumnTypes(#table({\"v\", \"d\"}, {{\"1.234,5\", \"31/01/2024\"}}), {{\"v\", type number}, {\"d\", type date}}, \"pt-BR\")";
    assert_eq!(run(&format!("{br}{{0}}")), "[v = 1234.5, d = #date(2024, 1, 31)]");
    assert_eq!(run("Table.AddColumn(#table({\"a\"}, {{1}, {0}}), \"b\", each if [a] = 0 then error \"zero\" else 1 / [a]){0}[b]"), "1");
    assert!(run("Table.AddColumn(#table({\"a\"}, {{1}, {0}}), \"b\", each if [a] = 0 then error \"zero\" else 1 / [a]){1}[b]").contains("zero"));
}

#[test]
fn table_headers_and_shape() {
    let raw = "#table({\"Column1\", \"Column2\"}, {{\"Name\", \"Age\"}, {\"Ana\", 30}, {\"Bo\", 41}})";
    assert_eq!(run(&format!("Table.PromoteHeaders({raw})")), "#table({\"Name\", \"Age\"}, {{\"Ana\", 30}, {\"Bo\", 41}})");
    assert_eq!(
        run("Table.ColumnNames(Table.PromoteHeaders(#table(3, {{\"a\", \"a\", 2024}}), [PromoteAllScalars = true]))"),
        "{\"a\", \"a_1\", \"2024\"}"
    );
    assert_eq!(run("Table.ColumnNames(Table.PromoteHeaders(#table(2, {{\"a\", 5}})))"), "{\"a\", \"Column2\"}");
    // One counter over every repeat, as Power Query numbers them.
    assert_eq!(
        run("Table.ColumnNames(Table.PromoteHeaders(#table(5, {{\"Total\", \"Total\", \"Name\", \"Name\", \"Total\"}})))"),
        "{\"Total\", \"Total_1\", \"Name\", \"Name_2\", \"Total_3\"}"
    );
    assert_eq!(run("Table.DemoteHeaders(#table({\"a\"}, {{1}}))"), "#table({\"Column1\"}, {{\"a\"}, {1}})");
    assert_eq!(run("Table.Transpose(#table({\"a\", \"b\"}, {{1, 2}, {3, 4}}))"), "#table({\"Column1\", \"Column2\"}, {{1, 3}, {2, 4}})");
    assert_eq!(
        run("Table.Combine({#table({\"a\", \"b\"}, {{1, 2}}), #table({\"b\", \"c\"}, {{3, 4}})})"),
        "#table({\"a\", \"b\", \"c\"}, {{1, 2, null}, {null, 3, 4}})"
    );
    assert_eq!(run("#table({\"a\"}, {{1}}) & #table({\"a\"}, {{2}})"), "#table({\"a\"}, {{1}, {2}})");
    assert_eq!(run("Table.Repeat(#table({\"a\"}, {{1}}), 2)[a]"), "{1, 1}");
    assert_eq!(run("Table.PrefixColumns(#table({\"a\"}, {}), \"T\")"), "#table({\"T.a\"}, {})");
    assert_eq!(run("Table.TransformColumnNames(#table({\"a b\"}, {}), Text.Upper)"), "#table({\"A B\"}, {})");
    assert_eq!(run("Table.AlternateRows(#table({\"a\"}, {{1}, {2}, {3}, {4}, {5}}), 1, 1, 1)[a]"), "{1, 3, 5}");
    assert_eq!(run("Table.InsertRows(#table({\"a\"}, {{1}, {3}}), 1, {[a = 2]})[a]"), "{1, 2, 3}");
}

#[test]
fn group_join_pivot() {
    assert_eq!(
        sales("Table.Group(Sales, {\"Region\"}, {{\"Rows\", each Table.RowCount(_), Int64.Type}, {\"Qty\", each List.Sum([Qty]), type number}})"),
        "#table({\"Region\", \"Rows\", \"Qty\"}, {{\"North\", 2, 13}, {\"South\", 1, 4}, {\"East\", 1, null}})"
    );
    assert_eq!(sales("Table.Group(Sales, \"Product\", {\"n\", each Table.RowCount(_)})[n]"), "{2, 1, 1}");
    assert_eq!(
        sales("Table.Group(Sales, \"Region\", {\"n\", each Table.RowCount(_)}, GroupKind.Local)[Region]"),
        "{\"North\", \"South\", \"North\", \"East\"}"
    );
    assert_eq!(
        sales("Table.Group(Sales, {\"Region\", \"Product\"}, {})"),
        "#table({\"Region\", \"Product\"}, {{\"North\", \"Pen\"}, {\"South\", \"Pen\"}, {\"North\", \"Ink\"}, {\"East\", \"Pad\"}})"
    );

    let people = "#table({\"id\", \"name\"}, {{1, \"Ana\"}, {2, \"Bo\"}, {3, \"Cy\"}})";
    let orders = "#table({\"pid\", \"item\"}, {{1, \"pen\"}, {1, \"ink\"}, {3, \"pad\"}, {9, \"cap\"}})";
    let join = |kind: &str, tail: &str| run(&format!("Table.Join({people}, \"id\", {orders}, \"pid\"{kind}){tail}"));
    assert_eq!(
        join("", ""),
        "#table({\"id\", \"name\", \"pid\", \"item\"}, {{1, \"Ana\", 1, \"pen\"}, {1, \"Ana\", 1, \"ink\"}, {3, \"Cy\", 3, \"pad\"}})"
    );
    assert_eq!(join(", JoinKind.LeftOuter", "[item]"), "{\"pen\", \"ink\", null, \"pad\"}");
    assert_eq!(join(", JoinKind.RightOuter", "[name]"), "{\"Ana\", \"Ana\", \"Cy\", null}");
    assert_eq!(join(", JoinKind.FullOuter", "[id]"), "{1, 1, 2, 3, null}");
    assert_eq!(join(", JoinKind.LeftAnti", "[name]"), "{\"Bo\"}");
    assert_eq!(join(", JoinKind.RightAnti", "[item]"), "{\"cap\"}");

    let nested = format!("Table.NestedJoin({people}, {{\"id\"}}, {orders}, {{\"pid\"}}, \"o\", JoinKind.LeftOuter)");
    assert_eq!(run(&format!("List.Transform({nested}[o], Table.RowCount)")), "{2, 0, 1}");
    assert_eq!(
        run(&format!("Table.ExpandTableColumn({nested}, \"o\", {{\"item\"}}, {{\"Item\"}})")),
        "#table({\"id\", \"name\", \"Item\"}, {{1, \"Ana\", \"pen\"}, {1, \"Ana\", \"ink\"}, {2, \"Bo\", null}, {3, \"Cy\", \"pad\"}})"
    );
    assert_eq!(run(&format!("Table.RowCount(Table.NestedJoin({people}, \"id\", {orders}, \"pid\", \"o\", JoinKind.Inner))")), "2");
    assert_eq!(run(&format!("Table.AggregateTableColumn({nested}, \"o\", {{{{\"item\", List.Count, \"n\"}}}})[n]")), "{2, 0, 1}");
    // Nulls match nulls.
    assert_eq!(run("Table.RowCount(Table.Join(#table({\"a\"}, {{null}}), \"a\", #table({\"b\"}, {{null}}), \"b\"))"), "1");

    let long = "#table({\"k\", \"attr\", \"v\"}, {{\"x\", \"A\", 1}, {\"x\", \"B\", 2}, {\"y\", \"A\", 3}})";
    assert_eq!(
        run(&format!("Table.Pivot({long}, {{\"A\", \"B\"}}, \"attr\", \"v\")")),
        "#table({\"k\", \"A\", \"B\"}, {{\"x\", 1, 2}, {\"y\", 3, null}})"
    );
    assert_eq!(
        run(&format!("Table.Pivot({long} & {long}, {{\"A\", \"B\"}}, \"attr\", \"v\", List.Sum)")),
        "#table({\"k\", \"A\", \"B\"}, {{\"x\", 2, 4}, {\"y\", 6, null}})"
    );
    // Rows come out in the order of the other columns' values, not in the order met.
    assert_eq!(
        run(
            "Table.Pivot(#table({\"K\", \"Att\", \"V\"}, {{\"b\", \"x\", 1}, {\"a\", \"x\", 2}, {\"b\", \"y\", 3}}), {\"x\", \"y\"}, \"Att\", \"V\")"
        ),
        "#table({\"K\", \"x\", \"y\"}, {{\"a\", 2, null}, {\"b\", 1, 3}})"
    );
    let wide = "#table({\"k\", \"A\", \"B\"}, {{\"x\", 1, 2}, {\"y\", 3, null}})";
    assert_eq!(
        run(&format!("Table.UnpivotOtherColumns({wide}, {{\"k\"}}, \"attr\", \"v\")")),
        "#table({\"k\", \"attr\", \"v\"}, {{\"x\", \"A\", 1}, {\"x\", \"B\", 2}, {\"y\", \"A\", 3}})"
    );
    assert_eq!(run(&format!("Table.Unpivot({wide}, {{\"A\", \"B\"}}, \"attr\", \"v\")[v]")), "{1, 2, 3}");
}

#[test]
fn expand_split_combine() {
    assert_eq!(
        run("Table.ExpandRecordColumn(#table({\"k\", \"r\"}, {{1, [a = 1, b = 2]}, {2, null}}), \"r\", {\"a\", \"b\"}, {\"A\", \"B\"})"),
        "#table({\"k\", \"A\", \"B\"}, {{1, 1, 2}, {2, null, null}})"
    );
    assert_eq!(
        run("Table.ExpandListColumn(#table({\"k\", \"l\"}, {{1, {\"a\", \"b\"}}, {2, {}}, {3, null}}), \"l\")"),
        "#table({\"k\", \"l\"}, {{1, \"a\"}, {1, \"b\"}, {2, null}, {3, null}})"
    );
    assert_eq!(
        run(
            "Table.SplitColumn(#table({\"n\", \"x\"}, {{\"a b c\", 1}, {\"d e\", 2}, {null, 3}}), \"n\", Splitter.SplitTextByDelimiter(\" \"), {\"p\", \"q\"})"
        ),
        "#table({\"p\", \"q\", \"x\"}, {{\"a\", \"b\", 1}, {\"d\", \"e\", 2}, {null, null, 3}})"
    );
    assert_eq!(
        run("Table.ColumnNames(Table.SplitColumn(#table({\"n\"}, {{\"a-b-c\"}}), \"n\", Splitter.SplitTextByDelimiter(\"-\")))"),
        "{\"n.1\", \"n.2\", \"n.3\"}"
    );
    assert_eq!(
        run(
            "Table.CombineColumns(#table({\"a\", \"b\", \"c\"}, {{\"x\", \"y\", 1}}), {\"a\", \"b\"}, Combiner.CombineTextByDelimiter(\" \"), \"ab\")"
        ),
        "#table({\"ab\", \"c\"}, {{\"x y\", 1}})"
    );
    check(&[
        ("Splitter.SplitTextByDelimiter(\",\")(\"a,\"\"b,c\"\",d\")", "{\"a\", \"b,c\", \"d\"}"),
        ("Splitter.SplitTextByDelimiter(\",\", QuoteStyle.None)(\"a,\"\"b,c\"\"\")", "{\"a\", \"\"\"b\", \"c\"\"\"}"),
        ("Splitter.SplitTextByAnyDelimiter({\",\", \";\"})(\"a,b;c\")", "{\"a\", \"b\", \"c\"}"),
        ("Splitter.SplitTextByEachDelimiter({\"-\", \":\"})(\"a-b:c-d\")", "{\"a\", \"b\", \"c-d\"}"),
        ("Splitter.SplitTextByPositions({0, 2, 3})(\"abcdef\")", "{\"ab\", \"c\", \"def\"}"),
        ("Splitter.SplitTextByLengths({1, 2})(\"abcdef\")", "{\"a\", \"bc\"}"),
        ("Splitter.SplitTextByRepeatedLengths(2)(\"abcde\")", "{\"ab\", \"cd\", \"e\"}"),
        ("Splitter.SplitTextByWhitespace()(\"a  b#(tab)c\")", "{\"a\", \"b\", \"c\"}"),
        ("Splitter.SplitTextByCharacterTransition({\"a\"..\"z\"}, {\"A\"..\"Z\"})(\"camelCaseText\")", "{\"camel\", \"Case\", \"Text\"}"),
        ("Combiner.CombineTextByDelimiter(\"-\")({\"a\", null, 1})", "\"a--1\""),
        ("Combiner.CombineTextByDelimiter(\",\")({\"a,b\", \"c\"})", "\"\"\"a,b\"\",c\""),
    ]);
}

#[test]
fn dates_and_times() {
    check(&[
        ("#date(2024, 2, 29)", "#date(2024, 2, 29)"),
        ("#date(2024, 1, 31) + #duration(1, 0, 0, 0)", "#date(2024, 2, 1)"),
        ("#date(2024, 3, 1) - #date(2024, 2, 1)", "#duration(29, 0, 0, 0)"),
        ("#datetime(2024, 1, 1, 10, 30, 0) + #duration(0, 14, 0, 0)", "#datetime(2024, 1, 2, 0, 30, 0)"),
        ("#date(2024, 1, 1) & #time(8, 15, 0)", "#datetime(2024, 1, 1, 8, 15, 0)"),
        ("#duration(1, 2, 3, 4) * 2", "#duration(2, 4, 6, 8)"),
        ("#duration(1, 0, 0, 0) / #duration(0, 6, 0, 0)", "4"),
        ("#date(2024, 1, 1) < #date(2024, 1, 2)", "true"),
        ("Date.Year(#date(2024, 5, 17))", "2024"),
        ("Date.Month(#date(2024, 5, 17))", "5"),
        ("Date.Day(#datetime(2024, 5, 17, 1, 2, 3))", "17"),
        ("Date.QuarterOfYear(#date(2024, 5, 17))", "2"),
        ("Date.DayOfYear(#date(2024, 3, 1))", "61"),
        ("Date.DaysInMonth(#date(2023, 2, 1))", "28"),
        ("Date.IsLeapYear(#date(2024, 1, 1))", "true"),
        ("Date.DayOfWeek(#date(2024, 5, 17), Day.Monday)", "4"),
        ("Date.DayOfWeekName(#date(2024, 5, 17))", "\"Friday\""),
        ("Date.DayOfWeekName(#date(2024, 5, 17), \"pt-BR\")", "\"sexta-feira\""),
        ("Date.MonthName(#date(2024, 5, 17))", "\"May\""),
        ("Date.MonthName(#date(2024, 5, 17), \"pt-BR\")", "\"maio\""),
        ("Date.WeekOfYear(#date(2024, 1, 7), Day.Sunday)", "2"),
        ("Date.AddDays(#date(2024, 2, 28), 2)", "#date(2024, 3, 1)"),
        ("Date.AddMonths(#date(2024, 1, 31), 1)", "#date(2024, 2, 29)"),
        ("Date.AddYears(#date(2024, 2, 29), 1)", "#date(2025, 2, 28)"),
        ("Date.AddQuarters(#date(2024, 1, 15), -1)", "#date(2023, 10, 15)"),
        ("Date.StartOfMonth(#date(2024, 5, 17))", "#date(2024, 5, 1)"),
        ("Date.EndOfMonth(#date(2024, 2, 10))", "#date(2024, 2, 29)"),
        ("Date.StartOfQuarter(#date(2024, 5, 17))", "#date(2024, 4, 1)"),
        ("Date.EndOfQuarter(#date(2024, 5, 17))", "#date(2024, 6, 30)"),
        ("Date.StartOfYear(#date(2024, 5, 17))", "#date(2024, 1, 1)"),
        ("Date.EndOfYear(#date(2024, 5, 17))", "#date(2024, 12, 31)"),
        ("Date.StartOfWeek(#date(2024, 5, 17), Day.Monday)", "#date(2024, 5, 13)"),
        ("Date.EndOfWeek(#date(2024, 5, 17), Day.Monday)", "#date(2024, 5, 19)"),
        ("Date.StartOfDay(#datetime(2024, 5, 17, 13, 0, 0))", "#datetime(2024, 5, 17, 0, 0, 0)"),
        ("Date.From(\"2024-05-17\")", "#date(2024, 5, 17)"),
        ("Date.From(\"5/17/2024\")", "#date(2024, 5, 17)"),
        ("Date.From(\"17/05/2024\", \"pt-BR\")", "#date(2024, 5, 17)"),
        ("Date.From(\"17 de maio de 2024\", \"pt-BR\")", "#date(2024, 5, 17)"),
        ("Date.From(\"May 17, 2024\")", "#date(2024, 5, 17)"),
        ("Date.From(45429)", "#date(2024, 5, 17)"),
        ("Date.From(#datetime(2024, 5, 17, 13, 0, 0))", "#date(2024, 5, 17)"),
        ("Date.FromText(\"20240517\", [Format = \"yyyyMMdd\"])", "#date(2024, 5, 17)"),
        ("Date.FromText(\"17.05.24\", [Format = \"dd.MM.yy\"])", "#date(2024, 5, 17)"),
        ("Date.ToText(#date(2024, 5, 7), \"yyyy-MM-dd\")", "\"2024-05-07\""),
        ("Date.ToText(#date(2024, 5, 7), \"d MMM yyyy\")", "\"7 May 2024\""),
        ("Date.ToText(#date(2024, 5, 7), [Format = \"dddd, d 'de' MMMM\", Culture = \"pt-BR\"])", "\"terça-feira, 7 de maio\""),
        ("Date.ToText(#date(2024, 5, 7))", "\"5/7/2024\""),
        ("Text.From(#date(2024, 5, 7), \"pt-BR\")", "\"07/05/2024\""),
        ("Text.From(#datetime(2024, 5, 7, 15, 4, 5))", "\"5/7/2024 3:04:05 PM\""),
        ("Text.From(#time(15, 4, 0))", "\"3:04 PM\""),
        ("Text.From(#duration(1, 2, 3, 4))", "\"1.02:03:04\""),
        ("DateTime.From(\"2024-05-17T13:45:10\")", "#datetime(2024, 5, 17, 13, 45, 10)"),
        ("DateTime.From(\"5/17/2024 1:45 PM\")", "#datetime(2024, 5, 17, 13, 45, 0)"),
        ("DateTime.Date(#datetime(2024, 5, 17, 13, 45, 10))", "#date(2024, 5, 17)"),
        ("DateTime.Time(#datetime(2024, 5, 17, 13, 45, 10))", "#time(13, 45, 10)"),
        ("DateTime.ToText(#datetime(2024, 5, 17, 13, 45, 10), \"HH:mm\")", "\"13:45\""),
        ("DateTimeZone.From(\"2024-05-17T13:45:10-03:00\")", "#datetimezone(2024, 5, 17, 13, 45, 10, -3, 0)"),
        ("DateTimeZone.ToUtc(#datetimezone(2024, 5, 17, 13, 0, 0, -3, 0))", "#datetimezone(2024, 5, 17, 16, 0, 0, 0, 0)"),
        ("DateTimeZone.RemoveZone(#datetimezone(2024, 5, 17, 13, 0, 0, -3, 0))", "#datetime(2024, 5, 17, 13, 0, 0)"),
        ("#datetimezone(2024, 5, 17, 13, 0, 0, -3, 0) = #datetimezone(2024, 5, 17, 16, 0, 0, 0, 0)", "true"),
        ("Time.Hour(#time(13, 45, 10))", "13"),
        ("Time.From(0.5)", "#time(12, 0, 0)"),
        ("Time.From(\"1:30 PM\")", "#time(13, 30, 0)"),
        ("Duration.TotalHours(#duration(1, 12, 0, 0))", "36"),
        ("Duration.Days(#duration(1, 12, 0, 0))", "1"),
        ("Duration.From(1.5)", "#duration(1, 12, 0, 0)"),
        ("Duration.FromText(\"1.02:03:04\")", "#duration(1, 2, 3, 4)"),
        ("Number.From(#datetime(2024, 5, 17, 12, 0, 0))", "45429.5"),
    ]);
    fails("#date(2024, 2, 30)", "isn't a date");
    fails("Date.From(\"not a date\")", "can't be read as a date");
    fails("DateTime.LocalNow()", "aren't available");
}

#[test]
fn csv_json_and_binary() {
    check(&[
        ("Csv.Document(\"a,b#(lf)1,2#(lf)\")", "#table({\"Column1\", \"Column2\"}, {{\"a\", \"b\"}, {\"1\", \"2\"}})"),
        ("Csv.Document(\"a;b#(cr,lf)1;\"\"x;y\"\"\", [Delimiter = \";\"])", "#table({\"Column1\", \"Column2\"}, {{\"a\", \"b\"}, {\"1\", \"x;y\"}})"),
        ("Csv.Document(\"a,b,c#(lf)1\")", "#table({\"Column1\", \"Column2\", \"Column3\"}, {{\"a\", \"b\", \"c\"}, {\"1\", null, null}})"),
        ("Csv.Document(\"a,b,c\", 2)", "#table({\"Column1\", \"Column2\"}, {{\"a\", \"b\"}})"),
        ("Csv.Document(\"x#(tab)y\", null, \"#(tab)\")", "#table({\"Column1\", \"Column2\"}, {{\"x\", \"y\"}})"),
        ("Csv.Document(\"\"\"multi#(lf)line\"\",2\")", "#table({\"Column1\", \"Column2\"}, {{\"multi#(lf)line\", \"2\"}})"),
        ("Table.PromoteHeaders(Csv.Document(\"n,v#(lf)a,1\"))", "#table({\"n\", \"v\"}, {{\"a\", \"1\"}})"),
        ("Json.Document(\"{\"\"b\"\": 1, \"\"a\"\": [true, null, \"\"x\"\", 1.5e2]}\")", "[b = 1, a = {true, null, \"x\", 150}]"),
        ("Json.Document(\"[{\"\"n\"\": \"\"caf\\u00e9 \\ud83d\\ude00\"\"}]\"){0}[n]", "\"café 😀\""),
        ("Table.FromRecords(Json.Document(\"[{\"\"a\"\":1,\"\"b\"\":2},{\"\"a\"\":3,\"\"b\"\":4}]\"))", "#table({\"a\", \"b\"}, {{1, 2}, {3, 4}})"),
        ("Text.FromBinary(Json.FromValue([a = {1, \"x\"}, d = #date(2024, 1, 2)]))", "\"{\"\"a\"\":[1,\"\"x\"\"],\"\"d\"\":\"\"2024-01-02\"\"}\""),
        ("Lines.FromText(\"a#(cr,lf)b#(lf)c\")", "{\"a\", \"b\", \"c\"}"),
        ("Binary.ToText(Text.ToBinary(\"hi!\"))", "\"aGkh\""),
        ("Text.FromBinary(Binary.FromText(\"aGkh\"))", "\"hi!\""),
        ("Binary.ToText(#binary({255, 0, 16}), BinaryEncoding.Hex)", "\"ff0010\""),
        ("Binary.Length(Text.ToBinary(\"é\"))", "2"),
        ("Binary.Length(Text.ToBinary(\"é\", TextEncoding.Windows))", "1"),
        ("Text.FromBinary(Text.ToBinary(\"€ café\", 1252), 1252)", "\"€ café\""),
        ("Text.FromBinary(Text.ToBinary(\"añ\", TextEncoding.Utf16), TextEncoding.Utf16)", "\"añ\""),
    ]);
    fails("Json.Document(\"{\"\"a\"\": }\")", "JSON isn't valid");
    fails("File.Contents(\"C:\\x.csv\")", "isn't available");
}

#[test]
fn values_and_expressions() {
    check(&[
        ("Value.Compare(1, 2)", "-1"),
        ("Value.Equals({1}, {1})", "true"),
        ("Value.FromText(\"12\")", "12"),
        ("Value.FromText(\"true\")", "true"),
        ("Value.FromText(\"2024-01-02\")", "#date(2024, 1, 2)"),
        ("Value.FromText(\"hello\")", "\"hello\""),
        ("Function.Invoke(Text.Start, {\"abc\", 2})", "\"ab\""),
        ("Expression.Evaluate(\"1 + 2\")", "3"),
        ("Expression.Evaluate(\"x * 2\", [x = 21])", "42"),
        ("Expression.Evaluate(\"Text.Upper(\"\"a\"\")\", #shared)", "\"A\""),
        ("Expression.Constant({1, \"a\"})", "\"{1, \"\"a\"\"}\""),
        ("Expression.Identifier(\"a b\")", "\"#\"\"a b\"\"\""),
        ("Record.HasFields(#shared, \"Table.SelectRows\")", "true"),
        ("Comparer.Ordinal(\"a\", \"b\")", "-1"),
        ("Comparer.FromCulture(\"en-US\", true)(\"A\", \"a\")", "0"),
        ("List.Contains({\"A\"}, \"a\", Comparer.FromCulture(\"en-US\", true))", "true"),
        ("Replacer.ReplaceText(\"aXb\", \"X\", \"-\")", "\"a-b\""),
        ("Diagnostics.Trace(0, \"m\", 5)", "5"),
        ("1 meta [note = \"x\"]", "1"),
    ]);
    // Names outside the given environment don't resolve.
    fails("Expression.Evaluate(\"Text.Upper(\"\"a\"\")\")", "isn't recognized");
}

#[test]
fn syntax_errors() {
    let err = |src: &str| crate::parse(src).err().map(|e| e.message).unwrap_or_default();
    assert!(err("1 +").contains("ends before"));
    assert!(err("let a = 1 b").contains("comma or in"));
    assert!(err("{1, 2").contains("comma or }"));
    assert!(err("\"open").contains("never closed"));
    assert!(err("1 2").contains("isn't expected here"));
    assert!(err("a $ b").contains("'$'"));
    assert!(err("let a = 1, a = 2 in a").contains("more than once"));
    assert!(err("/* open").contains("never closed"));
    let e = crate::parse("let\n  a = 1,\n  b = )\nin a").err().unwrap();
    assert_eq!(e.line_col("let\n  a = 1,\n  b = )\nin a"), (3, 7));
}

#[test]
fn steps_and_sections() {
    let q =
        "let\n    Source = {1, 2, 3},\n    #\"Kept Rows\" = List.Select(Source, each _ > 1),\n    Total = List.Sum(#\"Kept Rows\")\nin\n    Total";
    let s = crate::steps(q).unwrap();
    assert_eq!(s.steps.iter().map(|x| x.name.as_str()).collect::<Vec<_>>(), ["Source", "Kept Rows", "Total"]);
    assert_eq!(s.result.as_deref(), Some("Total"));
    assert_eq!(&q[s.steps[1].span.0..s.steps[1].span.1], "List.Select(Source, each _ > 1)");
    let mut it = Interp::new(&NoHost);
    assert_eq!(render(&it.eval_step(q, "Kept Rows").unwrap()), "{2, 3}");
    assert_eq!(render(&it.eval_step(q, "Source").unwrap()), "{1, 2, 3}");
    assert!(it.eval_step(q, "Nope").is_err());

    let section = "section Section1;\n\nshared Base = {1, 2};\n[ Description = \"doubles\" ]\nshared #\"My Query\" = let x = List.Transform(Base, each _ * 2) in x;\nHelper = (n) => n + 1;";
    let members = crate::parse_section(section).unwrap();
    assert_eq!(members.iter().map(|m| (m.name.as_str(), m.shared)).collect::<Vec<_>>(), [("Base", true), ("My Query", true), ("Helper", false)]);
    assert_eq!(members[1].formula, "let x = List.Transform(Base, each _ * 2) in x");

    // Queries refer to each other; a broken one only fails its users.
    let queries: Vec<(&str, &str)> = members
        .iter()
        .map(|m| (m.name.as_str(), m.formula.as_str()))
        .chain([("Broken", "let x = in"), ("UsesBroken", "Broken + 1"), ("Loop", "Loop")])
        .collect();
    it.set_queries(&queries);
    assert_eq!(render(&it.eval_query("My Query").unwrap()), "{2, 4}");
    assert_eq!(render(&it.eval_text("Helper(List.Sum(#\"My Query\"))").unwrap()), "7");
    assert!(it.eval_query("UsesBroken").unwrap_err().to_string().contains("syntax error at line 1"));
    assert!(it.eval_query("Loop").unwrap_err().to_string().contains("cyclic"));
    assert!(it.eval_query("Missing").is_err());
}

struct Workbook;
impl Host for Workbook {
    fn current_workbook(&self) -> Result<Vec<(String, Table)>, String> {
        let columns: crate::Names = [Rc::from("Item"), Rc::from("Qty")].into_iter().collect();
        let row = |item: &str, qty: f64| -> crate::Row { [Slot::Val(Value::text(item)), Slot::Val(Value::Number(qty))].into_iter().collect() };
        Ok(vec![("Stock".to_string(), Table::untyped(columns, vec![row("pen", 3.0), row("ink", 0.0), row("pad", 7.0)]))])
    }
    fn file_contents(&self, path: &str) -> Result<Vec<u8>, String> {
        if path.ends_with("data.csv") { Ok(b"name;amount\r\nA;1,5\r\nB;2,25\r\n".to_vec()) } else { Err(format!("The file '{path}' wasn't found.")) }
    }
    fn now(&self) -> Option<Now> {
        // 2024-05-17 10:00, UTC-3.
        Some(Now { local_ticks: (crate::datetime::days_from_civil(2024, 5, 17) * 24 + 10) * crate::datetime::TICKS_PER_HOUR, offset_minutes: -180 })
    }
    fn culture(&self) -> &str {
        "pt-BR"
    }
}

#[test]
fn host_data() {
    let run = |src: &str| run_with(&Workbook, src);
    // The shape of the query the editor writes for "From Table/Range".
    assert_eq!(
        run("let Source = Excel.CurrentWorkbook(){[Name = \"Stock\"]}[Content], Kept = Table.SelectRows(Source, each [Qty] > 0) in Kept"),
        "#table({\"Item\", \"Qty\"}, {{\"pen\", 3}, {\"pad\", 7}})"
    );
    assert_eq!(run("Excel.CurrentWorkbook()[Name]"), "{\"Stock\"}");
    // And for "From Text/CSV", read in the workbook's culture.
    let csv = "let Source = Csv.Document(File.Contents(\"C:\\data.csv\"), [Delimiter = \";\", Encoding = 65001]), Promoted = Table.PromoteHeaders(Source, [PromoteAllScalars = true]), Typed = Table.TransformColumnTypes(Promoted, {{\"name\", type text}, {\"amount\", type number}}) in Typed";
    assert_eq!(run(csv), "#table({\"name\", \"amount\"}, {{\"A\", 1.5}, {\"B\", 2.25}})");
    assert!(run("File.Contents(\"nope.txt\")").contains("wasn't found"));
    assert_eq!(run("try File.Contents(\"nope.txt\") catch (e) => e[Reason]"), "\"DataSource.Error\"");
    assert_eq!(run("DateTime.LocalNow()"), "#datetime(2024, 5, 17, 10, 0, 0)");
    assert_eq!(run("DateTimeZone.UtcNow()"), "#datetimezone(2024, 5, 17, 13, 0, 0, 0, 0)");
    assert_eq!(run("Date.IsInCurrentMonth(#date(2024, 5, 1))"), "true");
    assert_eq!(run("Date.IsInPreviousMonth(#date(2024, 4, 30))"), "true");
    assert_eq!(run("Date.IsInNextYear(#date(2025, 1, 1))"), "true");
    assert_eq!(run("Date.IsInCurrentWeek(#date(2024, 5, 12))"), "true");
    assert_eq!(run("Date.IsInYearToDate(#date(2024, 6, 1))"), "false");
    assert_eq!(run("Text.From(1.5)"), "\"1,5\"");
    assert_eq!(run("Culture.Current"), "\"pt-BR\"");
    let n = run("Number.Random()").parse::<f64>().unwrap();
    assert!((0.0..1.0).contains(&n));
}

/// Runs `f` on a thread with a 1 MiB stack, the size of the main thread on Windows: what
/// survives there survives in the app.
fn on_small_stack(f: impl FnOnce() + Send + 'static) {
    std::thread::Builder::new().stack_size(1 << 20).spawn(f).unwrap().join().unwrap();
}

#[test]
fn deep_nesting_is_an_error_not_a_crash() {
    on_small_stack(|| {
        // Syntax nested past the parser's limit.
        let deep = format!("{}1{}", "(".repeat(5_000), ")".repeat(5_000));
        assert!(crate::parse(&deep).unwrap_err().message.contains("nested too deeply"));
        let lists = format!("{}{}", "{".repeat(5_000), "}".repeat(5_000));
        assert!(crate::parse(&lists).is_err());
        let calls = format!("f{}", "(1)".repeat(5_000));
        assert!(crate::parse(&calls).unwrap_err().message.contains("too complex"));
        let nots = format!("{} true", "not ".repeat(5_000));
        assert!(crate::parse(&nots).is_err());
        // The deepest nesting the parser accepts evaluates, or fails cleanly.
        let ok = format!("{}1{}", "(".repeat(120), ")".repeat(120));
        assert_eq!(run(&ok), "1");
        let nested_lets = format!("{}1{}", "let a = (".repeat(50), ") in a".repeat(50));
        assert_eq!(run(&nested_lets), "1");
        // A long flat chain is fine: it doesn't nest.
        let chain = format!("0{}", " + 1".repeat(20_000));
        assert_eq!(run(&chain), "20000");
        // Runaway recursion stops at the depth limit.
        assert!(run("let f = (n) => @f(n + 1) in f(0)").contains("too deeply"));
        assert!(run("let f = (n) => 1 + @f(n + 1) in try f(0) otherwise 0").contains("too deeply"));
        assert!(run("let f = (l) => List.Transform(l, @f) in f({{{}}})") == "{{{}}}");
        assert!(run("let f = () => List.Sum(List.Transform({1}, each @f())) in f()").contains("too deeply"));
        // Recursion well within the limit works.
        assert_eq!(run("let sum = (n) => if n = 0 then 0 else n + @sum(n - 1) in sum(40)"), "820");
    });
}

#[test]
fn deep_values_drop_without_recursion() {
    on_small_stack(|| {
        // A list nested 200,000 levels deep, a record chain, a table chain and a closure chain:
        // building them is a loop, and so must dropping them be.
        assert_eq!(run("List.Count(List.Accumulate({1..200000}, {}, (s, c) => {s}))"), "1");
        assert_eq!(run("Record.FieldCount(List.Accumulate({1..200000}, [], (s, c) => [prev = s]))"), "1");
        assert_eq!(run("Table.RowCount(List.Accumulate({1..100000}, #table({\"t\"}, {}), (s, c) => #table({\"t\"}, {{s}})))"), "1");
        assert_eq!(run("let f = List.Accumulate({1..200000}, (x) => x, (f, c) => (x) => f(x) + c) in f is function"), "true");
        assert_eq!(run("(try error [Detail = List.Accumulate({1..200000}, {}, (s, c) => {s})])[HasError]"), "true");
        // Comparing and printing deep values gives up rather than recursing without end.
        assert!(run("let deep = List.Accumulate({1..5000}, {}, (s, c) => {s}) in deep = deep").starts_with("true"));
        assert!(
            run("let a = List.Accumulate({1..5000}, {}, (s, c) => {s}), b = List.Accumulate({1..5000}, {}, (s, c) => {s}) in a = b")
                .contains("too deeply")
        );
        assert!(run("List.Accumulate({1..5000}, {}, (s, c) => {s})").contains('…'));
        assert!(run("Text.FromBinary(Json.FromValue(List.Accumulate({1..5000}, {}, (s, c) => {s})))").contains("too deeply"));
        assert!(run(&format!("Json.Document(\"{}\")", "[".repeat(5_000))).contains("nests too deeply"));
    });
}

#[test]
fn sizes_are_bounded() {
    for src in [
        "{1..1e12}",
        "List.Numbers(0, 1e12)",
        "List.Repeat({1, 2}, 1e12)",
        "Text.Repeat(\"abc\", 1e12)",
        "Text.PadStart(\"a\", 1e12)",
        "List.Generate(() => 0, each true, each _ + 1)",
        "Table.Repeat(#table({\"a\"}, {{1}}), 1e12)",
        "List.Accumulate({1..40}, \"ab\", (s, c) => s & s)",
        "List.Dates(#date(2024, 1, 1), 1e12, #duration(1, 0, 0, 0))",
    ] {
        assert!(run(src).contains("too large"), "source: {src} gave {}", run(src));
    }
    // Limits aren't M errors: `try` doesn't hide them.
    assert!(run("try {1..1e12} otherwise 0").contains("too large"));
}

struct Stopper(std::cell::Cell<u32>);
impl Host for Stopper {
    fn cancelled(&self) -> bool {
        self.0.set(self.0.get() + 1);
        self.0.get() > 2
    }
}

#[test]
fn the_host_can_cancel() {
    let host = Stopper(std::cell::Cell::new(0));
    assert!(run_with(&host, "List.Sum(List.Transform({1..10000000}, each _ + 1))").contains("cancelled"));
}

#[test]
fn scopes_are_released() {
    // A function defined in a `let` holds its scope and the scope holds the function: evaluating
    // must not leak the table the scope also holds.
    let table = Rc::new(Table::untyped([Rc::from("a")].into_iter().collect(), vec![[Slot::Val(Value::Number(1.0))].into_iter().collect()]));
    struct One(Rc<Table>);
    impl Host for One {
        fn current_workbook(&self) -> Result<Vec<(String, Table)>, String> {
            Ok(vec![("T".into(), Table { columns: self.0.columns.clone(), types: self.0.types.clone(), rows: self.0.rows.clone() })])
        }
    }
    let host = One(table.clone());
    let row = table.rows[0].clone();
    let before = Rc::strong_count(&row);
    for src in [
        "let Source = Excel.CurrentWorkbook(){0}[Content], twice = (x) => x * 2, Out = Table.AddColumn(Source, \"b\", each twice([a])) in Out",
        "let Source = Excel.CurrentWorkbook(){0}[Content], fns = [f = (x) => @g(x), g = (x) => Table.RowCount(Source) + x] in fns[f](1)",
        "let Source = Excel.CurrentWorkbook(){0}[Content], rec = (n) => if n = 0 then Source else @rec(n - 1) in rec(5)",
        "let Source = Excel.CurrentWorkbook(){0}[Content], make = () => [t = Source, me = @make] in make()[me]()[t]",
    ] {
        let mut it = Interp::new(&host);
        assert!(it.eval_text(src).is_ok(), "source: {src}");
        drop(it);
        assert_eq!(Rc::strong_count(&row), before, "the scope of this query was never freed: {src}");
    }
}

#[test]
fn the_depth_limit_can_be_raised() {
    // A host that evaluates on a thread with a large stack allows deeper recursion.
    std::thread::Builder::new()
        .stack_size(64 << 20)
        .spawn(|| {
            let mut it = Interp::new(&NoHost);
            it.set_max_depth(20_000);
            let v = it.eval_text("let sum = (n) => if n = 0 then 0 else n + @sum(n - 1) in sum(3000)").unwrap();
            assert_eq!(render(&v), "4501500");
            assert!(it.eval_text("let f = (n) => @f(n + 1) in f(0)").unwrap_err().to_string().contains("too deeply"));
        })
        .unwrap()
        .join()
        .unwrap();
}

// ---------------------------------------------------------------- databases

/// A database in memory that logs what it is asked: the view `dbo.big` (which fails) and the
/// tables `dbo.orders` and `sales.orders`.
struct FakeDb {
    name: String,
    log: Rc<std::cell::RefCell<Vec<String>>>,
}

impl crate::Database for FakeDb {
    fn tables(&self) -> Result<Vec<crate::TableRef>, String> {
        self.log.borrow_mut().push("tables".into());
        let t = |schema: &str, name: &str, view| crate::TableRef { schema: schema.into(), name: name.into(), view };
        Ok(vec![t("dbo", "big", true), t("dbo", "orders", false), t("sales", "orders", false)])
    }
    fn table(&self, schema: &str, name: &str) -> Result<Table, String> {
        self.log.borrow_mut().push(format!("table {schema}.{name}"));
        if name == "big" {
            return Err("big is too big".into());
        }
        let row = |id: f64, who: &str| -> crate::Row { [Slot::Val(Value::Number(id)), Slot::Val(Value::text(who))].into_iter().collect() };
        Ok(Table {
            columns: [Rc::from("Id"), Rc::from("Who")].into_iter().collect(),
            types: vec![crate::Type::Number(crate::Facet::Int64), crate::Type::Text].into(),
            rows: vec![row(1.0, &self.name), row(2.0, schema)],
        })
    }
    fn query(&self, sql: &str) -> Result<Table, String> {
        self.log.borrow_mut().push(format!("query {sql}"));
        Ok(Table {
            columns: [Rc::from("sql")].into_iter().collect(),
            types: vec![crate::Type::Text].into(),
            rows: vec![[Slot::Val(Value::text(sql))].into_iter().collect()],
        })
    }
    fn databases(&self) -> Result<Vec<String>, String> {
        Ok(vec!["one".into(), "two".into()])
    }
    fn open(&self, database: &str) -> Result<Rc<dyn crate::Database>, String> {
        self.log.borrow_mut().push(format!("open {database}"));
        Ok(Rc::new(FakeDb { name: database.into(), log: self.log.clone() }))
    }
}

#[derive(Default)]
struct DbHost {
    log: Rc<std::cell::RefCell<Vec<String>>>,
}

impl Host for DbHost {
    fn database(&self, source: &crate::DataSource) -> Result<Rc<dyn crate::Database>, String> {
        self.log.borrow_mut().push(format!("connect {} {} {}", source.kind, source.server, source.database));
        if source.server == "nowhere" {
            return Err("Credentials are required to connect to nowhere.".into());
        }
        Ok(Rc::new(FakeDb { name: source.database.clone(), log: self.log.clone() }))
    }
}

#[test]
fn database_tables_are_read_only_when_opened() {
    let host = DbHost::default();
    let src = r#"let Source = Sql.Database("srv,1500", "shop"), Orders = Source{[Schema = "dbo", Item = "orders"]}[Data] in Orders"#;
    assert_eq!(run_with(&host, src), r#"#table({"Id", "Who"}, {{1, "shop"}, {2, "dbo"}})"#);
    // One connection, one list of tables, one table: not `big`, not `sales.orders`.
    assert_eq!(*host.log.borrow(), vec!["connect SqlServer srv,1500 shop", "tables", "table dbo.orders"]);

    let nav = r#"let Source = PostgreSQL.Database("pg", "shop") in Table.SelectColumns(Source, {"Name", "Schema", "Item", "Kind"})"#;
    assert_eq!(
        run_with(&host, nav),
        r#"#table({"Name", "Schema", "Item", "Kind"}, {{"dbo.big", "dbo", "big", "View"}, {"dbo.orders", "dbo", "orders", "Table"}, {"sales.orders", "sales", "orders", "Table"}})"#
    );
    assert!(!host.log.borrow().iter().any(|l| l.contains("big")), "listing doesn't read tables");
    // A table that fails fails only where it is read.
    assert_eq!(run_with(&host, r#"Table.RowCount(MySQL.Database("m", "d"))"#), "3");
    let failed = run_with(&host, r#"try MySQL.Database("m", "d"){[Item = "big"]}[Data]"#);
    assert!(failed.contains(r#"Reason = "DataSource.Error", Message = "big is too big""#), "{failed}");
    // Reading a table twice fetches it once.
    host.log.borrow_mut().clear();
    let twice = r#"let S = Sql.Database("s", "d"), T = S{1}[Data] in Table.RowCount(T) + Table.RowCount(S{1}[Data])"#;
    assert_eq!(run_with(&host, twice), "4");
    assert_eq!(host.log.borrow().iter().filter(|l| l.starts_with("table ")).count(), 1, "{:?}", host.log.borrow());
    // Tables in the Data column work with the table functions.
    let all = r#"Table.ExpandTableColumn(Table.SelectRows(Sql.Database("s", "d"), each [Item] = "orders"), "Data", {"Who"})[Who]"#;
    assert_eq!(run_with(&host, all), r#"{"d", "dbo", "d", "sales"}"#);
    let counts = r#"Table.TransformColumns(Table.SelectRows(Sql.Database("s", "d"), each [Kind] = "Table"), {"Data", Table.RowCount})[Data]"#;
    assert_eq!(run_with(&host, counts), "{2, 2}");
}

#[test]
fn databases_run_native_queries() {
    let host = DbHost::default();
    assert_eq!(run_with(&host, r#"Sql.Database("s", "d", [Query = "select 1"])"#), r#"#table({"sql"}, {{"select 1"}})"#);
    let pg = r#"Value.NativeQuery(PostgreSQL.Database("p", "d"), "select 2", null, [EnableFolding = true])"#;
    assert_eq!(run_with(&host, pg), r#"#table({"sql"}, {{"select 2"}})"#);
    let lite = r#"let S = SQLite.Database("C:\data\shop.db") in Value.NativeQuery(S, "select 3")"#;
    assert_eq!(run_with(&host, lite), r#"#table({"sql"}, {{"select 3"}})"#);
    assert!(host.log.borrow().iter().any(|l| l == r"connect SQLite C:\data\shop.db "));
    // Only a database runs SQL; parameters aren't supported.
    assert!(run_with(&host, r#"Value.NativeQuery(#table({"a"}, {}), "select 1")"#).contains("A native query runs on a database"));
    assert!(run_with(&host, r#"Value.NativeQuery(Sql.Database("s", "d"), "select @a", [a = 1])"#).contains("Parameters"));
    assert!(run_with(&host, r#"Sql.Database("s", "d", [Query = 1])"#).starts_with("ERROR"));
}

#[test]
fn databases_list_and_nest() {
    let host = DbHost::default();
    let h = r#"let S = Sql.Database("s", "d", [HierarchicalNavigation = true]) in {S[Name], S{[Name = "sales"]}[Data][Name], S{[Name = "dbo"]}[Data]{[Item = "orders"]}[Data][Who]}"#;
    assert_eq!(run_with(&host, h), r#"{{"dbo", "sales"}, {"orders"}, {"d", "dbo"}}"#);
    let dbs = r#"let S = Sql.Databases("s") in {S[Name], S{[Name = "two"]}[Data]{[Schema = "dbo", Item = "orders"]}[Data]{0}[Who]}"#;
    assert_eq!(run_with(&host, dbs), r#"{{"one", "two"}, "two"}"#);
    assert!(host.log.borrow().iter().any(|l| l == "open two") && !host.log.borrow().iter().any(|l| l == "open one"));
    // The host's refusal is the query's error; hosts without databases refuse every one.
    assert_eq!(run_with(&host, r#"try Sql.Database("nowhere", "d") otherwise "refused""#), r#""refused""#);
    assert!(run_with(&host, r#"Sql.Database("nowhere", "d")"#).contains("Credentials are required"));
    assert!(run_with(&NoHost, r#"Sql.Database("s", "d")"#).contains("Databases aren't available here"));
    assert!(run_with(&host, r#"Sql.Database("", "d")"#).contains("The server is missing"));
}

#[test]
fn lazy_cells_print_without_reading_and_cannot_read_themselves() {
    let host = DbHost::default();
    // Printing a navigation table doesn't fetch its tables.
    let shown = run_with(&host, r#"Sql.Database("s", "d")"#);
    assert!(shown.contains(r#""dbo.orders""#), "{shown}");
    assert!(!host.log.borrow().iter().any(|l| l.starts_with("table ")), "{:?}", host.log.borrow());
    // A lazy value that reads itself is an error, not a hang.
    let me: Rc<std::cell::RefCell<Option<Rc<crate::Lazy>>>> = Rc::default();
    let inner = me.clone();
    let lazy = crate::Lazy::new(move || match inner.borrow().as_ref() {
        Some(l) => l.force().clone(),
        None => Ok(Value::Null),
    });
    *me.borrow_mut() = Some(lazy.clone());
    assert!(lazy.force().is_err());
    me.borrow_mut().take();
}

// ---------------------------------------------------------------- folders, XML, the web

/// A host with a folder of two files and a web of canned answers; it logs what it reads.
#[derive(Default)]
struct SourcesHost {
    log: Rc<std::cell::RefCell<Vec<String>>>,
}

impl Host for SourcesHost {
    fn folder_files(&self, path: &str) -> Result<Vec<crate::FileEntry>, String> {
        if path != "C:\\data" {
            return Err(format!("There is no folder '{path}'."));
        }
        let file = |name: &str, body: &'static str| {
            let log = self.log.clone();
            let n = name.to_string();
            crate::FileEntry {
                name: name.into(),
                folder: "C:\\data\\".into(),
                size: body.len() as u64,
                accessed: None,
                modified: Some(20468 * 864_000_000_000),
                created: None,
                hidden: false,
                read_only: false,
                content: Rc::new(move || {
                    log.borrow_mut().push(format!("read {n}"));
                    Ok(body.as_bytes().to_vec())
                }),
            }
        };
        Ok(vec![file("a.csv", "x,y\n1,2"), file("notes", "hello")])
    }
    fn web(&self) -> Option<crate::Fetch> {
        let log = self.log.clone();
        Some(Rc::new(move |url: &str, headers: &[(String, String)]| {
            log.borrow_mut().push(format!("GET {url} {headers:?}"));
            let body = match url {
                "https://svc.test/odata/" | "https://svc.test/odata" => r#"{"value": [{"name": "People", "kind": "EntitySet", "url": "People"}]}"#,
                "https://svc.test/odata/People" => {
                    r#"{"value": [{"Id": 1, "Name": "Ana"}], "@odata.nextLink": "https://svc.test/odata/People?page=2"}"#
                }
                "https://svc.test/odata/People?page=2" => r#"{"value": [{"Id": 2, "Name": "Bo", "@odata.etag": "x"}]}"#,
                u if u.starts_with("https://api.test/") => r#"{"ok": true}"#,
                _ => return Err(format!("404 for {url}")),
            };
            Ok(body.as_bytes().to_vec())
        }))
    }
}

#[test]
fn xml_documents_and_tables() {
    let x = r#"<root xmlns:p="urn:p"><item id="1"><a>1</a><b>x &amp; y</b></item><item id="2"><a>2</a><b><![CDATA[<z>]]></b></item><meta><v>9</v><v>10</v></meta></root>"#;
    let q = |e: &str| run(&e.replace("@X", &crate::quote_text(x)));
    assert_eq!(q("Xml.Tables(@X)[Name]"), r#"{"item", "meta"}"#);
    assert_eq!(q("Xml.Tables(@X){0}[Table]"), r#"#table({"Attribute:id", "a", "b"}, {{"1", "1", "x & y"}, {"2", "2", "<z>"}})"#);
    // Repeated children make a table of their own.
    assert_eq!(q("Xml.Tables(@X){1}[Table]{0}[v]"), r#"#table({"Element:Text"}, {{"9"}, {"10"}})"#);
    assert_eq!(q("Xml.Document(@X){0}[Name]"), r#""root""#);
    assert_eq!(q("Xml.Document(@X){0}[Value]{0}[Attributes]{0}[Value]"), r#""1""#);
    assert_eq!(q("Xml.Document(@X){0}[Value]{0}[Value]{1}[Value]"), r#""x & y""#);
    // Binary content works like text.
    assert_eq!(q("Table.RowCount(Xml.Tables(Text.ToBinary(@X)))"), "2");
    // Entity tricks and broken documents are errors, not crashes.
    let laughs = r#"<?xml version="1.0"?><!DOCTYPE lolz [<!ENTITY lol "lol"><!ENTITY lol2 "&lol;&lol;">]><lolz>&lol2;</lolz>"#;
    assert!(run(&format!("Xml.Document({})", crate::quote_text(laughs))).starts_with("ERROR"));
    assert!(run(r#"Xml.Tables("<a><b></a>")"#).starts_with("ERROR"));
    assert!(run(r#"Xml.Tables("<a/><b/>")"#).starts_with("ERROR"));
    let deep = format!("{}{}", "<a>".repeat(5000), "</a>".repeat(5000));
    assert!(run(&format!("Xml.Document({})", crate::quote_text(&deep))).starts_with("ERROR"));
}

#[test]
fn html_tables_of_a_page() {
    let html = r#"<html><head><script>var s = "<table><tr><td>no</td></tr></table>";</script></head><body>
        <table class="data" id="t1"><caption>Vendas &amp; metas</caption>
          <tr><th>Região</th><th>Q1</th><th>Q2</th></tr>
          <tr><td rowspan="2">Norte</td><td>1</td><td>2</td>
          <tr><td>3</td><td>4</td></tr>
          <tr><td colspan="2">Total&nbsp;geral</td><td>10</td></tr>
        </table>
        <!-- <table><tr><td>hidden</td></tr></table> -->
        <table><tr><td>a<br>b</td><td><table><tr><td>inner</td></tr></table></td></tr></table>
        </body></html>"#;
    let q = |e: &str| run(&e.replace("@H", &crate::quote_text(html)));
    assert_eq!(q("Table.RowCount(Web.Page(@H))"), "3");
    assert_eq!(q("Web.Page(@H){0}[Caption]"), r#""Vendas & metas""#);
    assert_eq!(q("{Web.Page(@H){0}[ClassName], Web.Page(@H){0}[Id]}"), r#"{"data", "t1"}"#);
    assert_eq!(
        q("Web.Page(@H){0}[Data]"),
        r#"#table({"Região", "Q1", "Q2"}, {{"Norte", "1", "2"}, {"Norte", "3", "4"}, {"Total geral", "Total geral", "10"}})"#
    );
    assert_eq!(q("Web.Page(@H){1}[Data]{0}[Column1]"), r#""a b""#);
    assert_eq!(q("Web.Page(@H){2}[Data]"), r#"#table({"Column1"}, {{"inner"}})"#);
    assert_eq!(run(r#"Table.RowCount(Web.Page("no tables here"))"#), "0");
    assert!(run(&format!("Web.Page({})", crate::quote_text(&"<table>".repeat(1000)))).starts_with("ERROR"));
}

#[test]
fn folder_files_are_read_when_opened() {
    let host = SourcesHost::default();
    let src = r#"Table.SelectColumns(Folder.Files("C:\data"), {"Name", "Extension", "Folder Path"})"#;
    assert_eq!(run_with(&host, src), r#"#table({"Name", "Extension", "Folder Path"}, {{"a.csv", ".csv", "C:\data\"}, {"notes", "", "C:\data\"}})"#);
    assert!(host.log.borrow().is_empty(), "listing reads no file");
    let csv = r#"Table.PromoteHeaders(Csv.Document(Folder.Files("C:\data"){[Name = "a.csv"]}[Content]))"#;
    assert_eq!(run_with(&host, csv), r#"#table({"x", "y"}, {{"1", "2"}})"#);
    assert_eq!(*host.log.borrow(), vec!["read a.csv"]);
    assert_eq!(run_with(&host, r#"Folder.Files("C:\data"){0}[Attributes][Size]"#), "7");
    assert!(run_with(&host, r#"Folder.Files("Z:\none")"#).contains("There is no folder"));
    assert!(run(r#"Folder.Files("C:\data")"#).contains("Reading folders isn't available here"));
}

#[test]
fn web_contents_and_odata_feeds() {
    let host = SourcesHost::default();
    let get = r#"Json.Document(Web.Contents("https://api.test/v1", [RelativePath = "/items", Query = [q = "a b&c", n = "2"], Headers = [#"X-Key" = "k"]]))"#;
    assert_eq!(run_with(&host, get), "[ok = true]");
    assert_eq!(host.log.borrow().last().map(String::as_str), Some(r#"GET https://api.test/v1/items?q=a%20b%26c&n=2 [("X-Key", "k")]"#));
    assert!(run_with(&host, r#"Web.Contents("ftp://x")"#).contains("isn't a web address"));
    assert!(run_with(&host, r#"Web.Contents("https://api.test", [Content = Text.ToBinary("x")])"#).contains("only reads"));
    assert!(run(r#"Web.Contents("https://api.test")"#).contains("Reading from the web isn't available here"));
    assert!(run_with(&host, r#"Web.Contents("https://nowhere.test/")"#).contains("404"));

    // A service document lists entity sets; a set is read, page after page, when opened.
    host.log.borrow_mut().clear();
    assert_eq!(run_with(&host, r#"OData.Feed("https://svc.test/odata")[Name]"#), r#"{"People"}"#);
    assert_eq!(host.log.borrow().len(), 1, "listing reads no entity set");
    assert_eq!(
        run_with(&host, r#"OData.Feed("https://svc.test/odata"){[Name = "People"]}[Data]"#),
        r#"#table({"Id", "Name"}, {{1, "Ana"}, {2, "Bo"}})"#
    );
    assert_eq!(run_with(&host, r#"Table.RowCount(OData.Feed("https://svc.test/odata/People"))"#), "2");
}
