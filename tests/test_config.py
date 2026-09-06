import os
import tempfile
import unittest
from pathlib import Path

from llmsim.config import Config, ConfigError, deep_merge


class TestDeepMerge(unittest.TestCase):
    def test_nested_tables_merge(self):
        base = {"a": {"x": 1, "y": 2}, "b": 3}
        over = {"a": {"y": 20, "z": 30}}
        self.assertEqual(deep_merge(base, over), {"a": {"x": 1, "y": 20, "z": 30}, "b": 3})

    def test_lists_replace_rather_than_concatenate(self):
        self.assertEqual(deep_merge({"p": [1, 2, 3]}, {"p": [9]}), {"p": [9]})

    def test_base_is_not_mutated(self):
        base = {"a": {"x": 1}}
        deep_merge(base, {"a": {"x": 2}})
        self.assertEqual(base, {"a": {"x": 1}})


class TestConfigLoad(unittest.TestCase):
    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.dir = Path(self._tmp.name)

    def tearDown(self):
        self._tmp.cleanup()

    def write(self, name, text):
        p = self.dir / name
        p.write_text(text, encoding="utf-8")
        return p

    def test_load_and_dotted_access(self):
        p = self.write("s.toml", '[engine]\nmbu_decode = 0.7\nname = "vllm"\n')
        cfg = Config.load(p)
        self.assertAlmostEqual(cfg.float_("engine.mbu_decode"), 0.7)
        self.assertEqual(cfg.str_("engine.name"), "vllm")

    def test_missing_key_raises_with_source(self):
        p = self.write("s.toml", "[engine]\nx = 1\n")
        cfg = Config.load(p)
        with self.assertRaises(ConfigError) as ctx:
            cfg.get("engine.nope")
        self.assertIn("s.toml", str(ctx.exception))

    def test_missing_key_default(self):
        cfg = Config.load(self.write("s.toml", "[a]\nb = 1\n"))
        self.assertEqual(cfg.get("a.zz", 5), 5)

    def test_extends_merges_parent(self):
        self.write("base.toml", '[engine]\nmbu_decode = 0.7\nmfu = 0.45\n[router]\npolicy = "rr"\n')
        child = self.write("child.toml", 'extends = "base.toml"\n[router]\npolicy = "p2c"\n')
        cfg = Config.load(child)
        self.assertEqual(cfg.str_("router.policy"), "p2c")
        self.assertAlmostEqual(cfg.float_("engine.mbu_decode"), 0.7)
        self.assertNotIn("extends", cfg.as_dict())

    def test_extends_chain(self):
        self.write("a.toml", "[v]\nk = 1\nonly_a = true\n")
        self.write("b.toml", 'extends = "a.toml"\n[v]\nk = 2\n')
        c = self.write("c.toml", 'extends = "b.toml"\n[v]\nk = 3\n')
        cfg = Config.load(c)
        self.assertEqual(cfg.int_("v.k"), 3)
        self.assertTrue(cfg.bool_("v.only_a"))

    def test_missing_file_raises(self):
        with self.assertRaises(ConfigError):
            Config.load(self.dir / "absent.toml")

    def test_broken_extends_raises(self):
        c = self.write("c.toml", 'extends = "nope.toml"\n')
        with self.assertRaises(ConfigError):
            Config.load(c)

    def test_extends_cycle_is_bounded(self):
        self.write("x.toml", 'extends = "y.toml"\n')
        self.write("y.toml", 'extends = "x.toml"\n')
        with self.assertRaises(ConfigError):
            Config.load(self.dir / "x.toml")


class TestOverrides(unittest.TestCase):
    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.p = Path(self._tmp.name) / "s.toml"
        self.p.write_text('[router]\npolicy = "rr"\n[workload]\nrate_rps = 10\n', encoding="utf-8")

    def tearDown(self):
        self._tmp.cleanup()

    def test_override_types_follow_toml_literals(self):
        cfg = Config.load(
            self.p,
            overrides=[
                "router.policy=p2c",
                "workload.rate_rps=42.5",
                "engine.chunked=true",
                "engine.budget=8192",
                "engine.pools=[1, 2]",
            ],
        )
        self.assertEqual(cfg.str_("router.policy"), "p2c")
        self.assertAlmostEqual(cfg.float_("workload.rate_rps"), 42.5)
        self.assertTrue(cfg.bool_("engine.chunked"))
        self.assertEqual(cfg.int_("engine.budget"), 8192)
        self.assertEqual(cfg.get("engine.pools"), [1, 2])

    def test_override_creates_missing_tables(self):
        cfg = Config.load(self.p, overrides=["deep.nested.key=3"])
        self.assertEqual(cfg.int_("deep.nested.key"), 3)

    def test_malformed_override_raises(self):
        with self.assertRaises(ConfigError):
            Config.load(self.p, overrides=["not-an-assignment"])

    def test_scientific_notation_is_an_int_when_whole(self):
        cfg = Config.load(self.p, overrides=["kv.capacity=1.37e6"])
        self.assertEqual(cfg.int_("kv.capacity"), 1_370_000)

    def test_non_integer_rejected_by_int_accessor(self):
        cfg = Config.load(self.p, overrides=["kv.capacity=1.5"])
        with self.assertRaises(ConfigError):
            cfg.int_("kv.capacity")

    def test_section_returns_subconfig(self):
        cfg = Config.load(self.p)
        self.assertEqual(cfg.section("router").str_("policy"), "rr")

    def test_contains(self):
        cfg = Config.load(self.p)
        self.assertIn("router.policy", cfg)
        self.assertNotIn("router.absent", cfg)

    def test_get_returns_a_copy_so_config_is_effectively_read_only(self):
        cfg = Config.load(self.p)
        got = cfg.section("router").as_dict()
        got["policy"] = "mutated"
        self.assertEqual(cfg.str_("router.policy"), "rr")


if __name__ == "__main__":
    unittest.main()
